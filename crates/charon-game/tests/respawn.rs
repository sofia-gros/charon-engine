//! 仕様 23 §1: 死亡と復活。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::Definitions;
use charon_game::game::{GameEvent, GameInput, GameState};
use charon_game::respawn::RESPAWN_TICKS;
use charon_game::status::StatusKind;
use charon_game::vitals::{BREATH_MAX, MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld, PlayerInput};
use charon_physics::PhysicsEngine;

struct Arena {
    world: World,
    physics: PhysicsEngine,
    entities: EntityWorld,
    game: GameState,
    player: EntityId,
    tick: u64,
}

impl Arena {
    fn new() -> Self {
        let materials = MaterialTable::builtin().unwrap();
        let defs = Definitions::engine_defaults(&materials).unwrap();
        let mut world = World::new(Arc::new(materials), 1);
        world.insert_chunk(ChunkPos::new(0, 0));
        testkit::fill(&mut world, 0, 40, 63, 40, '#');
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(40 * 16, 10 * 16, &world).unwrap();
        let mut game = GameState::from_definitions(&defs);
        game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
        game.give_inventory(player).unwrap();
        Self { world, physics: PhysicsEngine::new(), entities, game, player, tick: 0 }
    }

    fn tick(&mut self, spawn: [i32; 2]) {
        self.entities.step_players(&[(self.player, PlayerInput::default())], &self.world).unwrap();
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
        self.game.step_respawn(&self.world, &mut self.entities, spawn);
        self.world.step();
        self.tick += 1;
    }
}

/// 倒れて `RESPAWN_TICKS` たつと、出発点で、体力・息・マナが満タンで復活する。持ち物はそのまま。状態異常は消える。
#[test]
fn a_dead_player_respawns_at_the_spawn_point_with_full_vitals() {
    let mut a = Arena::new();
    let spawn = [10, 16];
    let stone = a.world.materials().id("stone").unwrap();
    a.game.inventories.get_mut(&a.player).unwrap().add(stone, 7);
    for _ in 0..30 {
        a.tick(spawn);
    }
    assert_eq!(a.game.death_ticks(a.player), None, "生きている間は数えない");
    a.game.vitals.get_mut(a.player).unwrap().hp = 0;
    a.game.vitals.get_mut(a.player).unwrap().breath = 3;
    a.game.wands.get_mut(&a.player).unwrap().mana = 0;
    a.game.statuses.apply(a.player, StatusKind::Burning, 600);
    a.game.take_events();
    for tick in 1..RESPAWN_TICKS {
        a.tick(spawn);
        assert_eq!(a.game.death_ticks(a.player), Some(tick), "倒れてから数える");
        assert!(!a.game.vitals.get(a.player).unwrap().alive());
    }
    a.tick(spawn);
    let vitals = a.game.vitals.get(a.player).unwrap();
    assert_eq!((vitals.hp, vitals.breath), (MAX_HP, BREATH_MAX));
    assert_eq!(a.game.wands[&a.player].mana, a.game.wands[&a.player].mana_max);
    assert!(!a.game.statuses.has(a.player, StatusKind::Burning), "燃えていない");
    let e = a.entities.get(a.player).unwrap();
    assert_eq!((e.x16.div_euclid(16), e.y16.div_euclid(16)), (10, 16), "出発点");
    assert_eq!(a.game.inventories[&a.player].count(stone), 7, "持ち物はそのまま");
    assert_eq!(a.game.death_ticks(a.player), None);
    assert!(
        a.game
            .take_events()
            .iter()
            .any(|e| matches!(e, GameEvent::Respawned(id) if *id == a.player))
    );
}

/// 出発点が地形に埋まっていたら、少し上で復活する。どこにも置けなければ、次の tick にまた試す。
#[test]
fn respawn_moves_up_out_of_terrain_and_waits_when_it_cannot() {
    let mut a = Arena::new();
    // 出発点 (10, 30) の下半分が、石に埋まっている。
    testkit::fill(&mut a.world, 8, 36, 40, 39, '#');
    a.game.vitals.get_mut(a.player).unwrap().hp = 0;
    for _ in 0..RESPAWN_TICKS {
        a.tick([10, 30]);
    }
    assert!(a.game.vitals.get(a.player).unwrap().alive());
    let e = a.entities.get(a.player).unwrap();
    assert!(e.y16.div_euclid(16) <= 30 + 5, "石の上で復活する: {}", e.y16 / 16);

    // どこにも置けない (出発点のまわりが、上まで石)。倒れたまま、待ち続ける。
    let mut b = Arena::new();
    testkit::fill(&mut b.world, 0, 0, 63, 39, '#');
    b.game.vitals.get_mut(b.player).unwrap().hp = 0;
    for _ in 0..RESPAWN_TICKS + 20 {
        b.tick([10, 30]);
    }
    assert!(!b.game.vitals.get(b.player).unwrap().alive(), "置けないので、倒れたまま");
    assert!(b.game.death_ticks(b.player).is_some_and(|t| t >= RESPAWN_TICKS));
}

/// 敵は復活しない。
#[test]
fn enemies_do_not_respawn() {
    let mut a = Arena::new();
    let materials = a.world.materials().clone();
    let defs = Definitions::engine_defaults(&materials).unwrap();
    let enemy = a.game.spawn_enemy(&defs, &mut a.entities, &a.world, "walker", (10, 10)).unwrap();
    a.game.vitals.get_mut(enemy).unwrap().hp = 0;
    for _ in 0..RESPAWN_TICKS + 10 {
        a.tick([10, 16]);
    }
    assert!(!a.game.enemies.contains_key(&enemy), "倒した敵は撤去される");
    assert_eq!(a.game.death_ticks(enemy), None);
}

/// 同じ入力なら、同じ結果 (復活を含めて)。
#[test]
fn respawn_is_deterministic() {
    let run = || {
        let mut a = Arena::new();
        for tick in 0..400 {
            if tick == 20 {
                a.game.vitals.get_mut(a.player).unwrap().hp = 0;
            }
            a.tick([10, 16]);
        }
        (a.game.state_hash(), a.entities.state_hash(), a.world.state_hash())
    };
    assert_eq!(run(), run());
}
