//! 仕様 10 §17: 状態異常 (燃える・濡れる・毒)。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::Definitions;
use charon_game::game::{GameInput, GameState};
use charon_game::status::{BURN_INTERVAL, POISON_INTERVAL, StatusKind, TICKS_MAX, WET_TICKS};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld};
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
    /// 空の 64×64 の世界の (10, 50) あたりに、プレイヤー (5×10) が立つ。
    fn new() -> Self {
        let materials = MaterialTable::builtin().unwrap();
        let defs = Definitions::engine_defaults(&materials).unwrap();
        let mut world = World::new(Arc::new(materials), 1);
        world.insert_chunk(ChunkPos::new(0, 0));
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(10 * 16, 26 * 16, &world).unwrap();
        let mut game = GameState::from_definitions(&defs);
        game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
        Self { world, physics: PhysicsEngine::new(), entities, game, player, tick: 0 }
    }

    fn step(&mut self) {
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
    }

    fn hp(&self) -> i32 {
        self.game.vitals.get(self.player).unwrap().hp
    }

    /// プレイヤーの体の中に 1 セル置く。
    fn place(&mut self, ch: char) {
        testkit::place(&mut self.world, 12, 43, ch);
    }

    fn has(&self, kind: StatusKind) -> bool {
        self.game.statuses.has(self.player, kind)
    }
}

/// 燃えると `BURN_INTERVAL` tick ごとに 1 ダメージ、毒は `POISON_INTERVAL` tick ごとに 1 ダメージ。濡れるだけではダメージがない。
#[test]
fn burning_and_poison_deal_damage_on_a_beat_and_expire() {
    let mut arena = Arena::new();
    let id = arena.player;
    assert!(arena.game.statuses.apply(id, StatusKind::Burning, 4 * BURN_INTERVAL));
    for _ in 0..4 * BURN_INTERVAL {
        arena.step();
    }
    assert_eq!(arena.hp(), MAX_HP - 4);
    assert!(!arena.has(StatusKind::Burning), "時間が来たら消える");

    assert!(arena.game.statuses.apply(id, StatusKind::Poison, 3 * POISON_INTERVAL));
    for _ in 0..3 * POISON_INTERVAL {
        arena.step();
    }
    assert_eq!(arena.hp(), MAX_HP - 4 - 3);

    assert!(arena.game.statuses.apply(id, StatusKind::Wet, 100));
    for _ in 0..100 {
        arena.step();
    }
    assert_eq!(arena.hp(), MAX_HP - 7, "濡れているだけではダメージがない");
}

/// 液体に浸かると濡れ、燃えているのが消える。濡れているあいだは燃えない。火のセルに触れると燃える。
#[test]
fn liquid_wets_and_extinguishes_fire_touches_ignite() {
    let mut arena = Arena::new();
    arena.place('f');
    arena.step();
    assert!(arena.has(StatusKind::Burning), "火に触れて燃える");
    assert!(!arena.has(StatusKind::Wet));
    assert!(
        arena.hp() < MAX_HP || arena.game.statuses.remaining(arena.player, StatusKind::Burning) > 0
    );

    arena.place('w');
    arena.step();
    assert!(arena.has(StatusKind::Wet), "液体で濡れる");
    assert!(!arena.has(StatusKind::Burning), "濡れたら火が消える");
    assert!(arena.game.statuses.remaining(arena.player, StatusKind::Wet) <= WET_TICKS);
    // 濡れている間は、火のセルに触れても燃えない。
    arena.place('f');
    arena.step();
    assert!(!arena.has(StatusKind::Burning));
    // 濡れが乾いて、火のそばにいれば、また燃える (残りが尽きるまで待つ)。
    // (火のセルの中にいるので、体力が尽きて状態が消えないように、毎 tick 満タンにする。)
    let id = arena.player;
    for _ in 0..=WET_TICKS {
        arena.game.vitals.get_mut(id).unwrap().hp = MAX_HP;
        arena.step();
    }
    assert!(!arena.has(StatusKind::Wet));
    arena.game.vitals.get_mut(id).unwrap().hp = MAX_HP;
    arena.step();
    assert!(arena.has(StatusKind::Burning), "乾いた次の tick に、火で燃える");
}

/// 長さは上限に収まり、0 は何もせず、延ばすときは長い方になる。倒れたら状態は消える。
#[test]
fn durations_are_clamped_and_extended_and_death_clears() {
    let mut arena = Arena::new();
    let id = arena.player;
    let statuses = &mut arena.game.statuses;
    assert!(!statuses.apply(id, StatusKind::Poison, 0));
    assert!(statuses.apply(id, StatusKind::Poison, u16::MAX));
    assert_eq!(statuses.remaining(id, StatusKind::Poison), TICKS_MAX);
    statuses.clear(id);
    statuses.apply(id, StatusKind::Poison, 50);
    statuses.apply(id, StatusKind::Poison, 20);
    assert_eq!(statuses.remaining(id, StatusKind::Poison), 50, "短い方では縮まない");
    statuses.apply(id, StatusKind::Poison, 80);
    assert_eq!(statuses.remaining(id, StatusKind::Poison), 80);

    arena.game.vitals.get_mut(id).unwrap().damage(MAX_HP);
    arena.step();
    assert!(!arena.has(StatusKind::Poison), "倒れたら消える");
}

/// 同じ手順なら、結果 (体力と残り) が同じになる。
#[test]
fn statuses_are_deterministic() {
    let run = || {
        let mut arena = Arena::new();
        arena.game.statuses.apply(arena.player, StatusKind::Poison, 200);
        arena.place('f');
        let mut trace = Vec::new();
        for _ in 0..300 {
            arena.step();
            trace.push((
                arena.hp(),
                arena.game.statuses.remaining(arena.player, StatusKind::Burning),
                arena.game.statuses.remaining(arena.player, StatusKind::Poison),
            ));
        }
        trace
    };
    assert_eq!(run(), run());
}
