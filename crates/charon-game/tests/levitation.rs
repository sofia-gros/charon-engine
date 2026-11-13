//! 仕様 10 §18: 浮遊 (燃料つきのジェット)。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::Definitions;
use charon_game::game::GameState;
use charon_game::levitation::{Levitation, LevitationDef};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld, PlayerInput};

/// 燃料が 1 秒ぶんの浮遊 (ふつうのジャンプとの違いが見える長さ)。
const SHORT: LevitationDef = LevitationDef { fuel_max: 60, thrust: 8, rise_max: 40, recharge: 3 };

struct Arena {
    world: World,
    entities: EntityWorld,
    game: GameState,
    player: EntityId,
}

impl Arena {
    /// 高さのある 64×64 の世界。床は y = 62。プレイヤーは床の上に立つ。
    fn new(def: Option<LevitationDef>) -> Self {
        let materials = MaterialTable::builtin().unwrap();
        let defs = Definitions::engine_defaults(&materials).unwrap();
        let mut world = World::new(Arc::new(materials), 1);
        // 上にも十分な高さ (ふつうのジャンプでも約 70 セル上がるので、天井にぶつからないように)。
        for cy in -4..=0 {
            world.insert_chunk(ChunkPos::new(0, cy));
        }
        testkit::fill(&mut world, 0, 62, 63, 62, '#');
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(10 * 16, 50 * 16, &world).unwrap();
        let mut game = GameState::from_definitions(&defs);
        game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
        if let Some(def) = def {
            game.equip_levitation(player, def);
        }
        let mut arena = Self { world, entities, game, player };
        for _ in 0..40 {
            arena.tick(false);
        }
        assert!(arena.entities.get(player).unwrap().grounded, "床に立つ");
        arena
    }

    /// 1 tick: 移動のあとに浮遊。
    fn tick(&mut self, held: bool) -> bool {
        self.entities
            .step_players(&[(self.player, PlayerInput { horizontal: 0, jump: held })], &self.world)
            .unwrap();
        self.game.step_levitation(&mut self.entities, self.player, held)
    }

    fn y16(&self) -> i64 {
        self.entities.get(self.player).unwrap().y16
    }

    fn fuel(&self) -> u16 {
        self.game.levitation[&self.player].fuel
    }

    /// ボタンを `ticks` の間押し続けて、いちばん高く (y16 が小さく) なった位置。
    fn hold_for(&mut self, ticks: usize) -> i64 {
        let mut top = self.y16();
        for _ in 0..ticks {
            self.tick(true);
            top = top.min(self.y16());
        }
        top
    }
}

/// 押し続けると、ふつうのジャンプよりずっと高く上がる。燃料が尽きると落ちる。
#[test]
fn holding_the_button_rises_higher_than_a_jump_until_the_fuel_runs_out() {
    let start = Arena::new(None).y16();
    let mut plain = Arena::new(None);
    let plain_top = plain.hold_for(120);
    let mut jet = Arena::new(Some(SHORT));
    let jet_top = jet.hold_for(120);
    assert!(
        start - jet_top > (start - plain_top) * 2,
        "ふつう {} 、浮遊 {}",
        start - plain_top,
        start - jet_top
    );

    assert_eq!(jet.fuel(), 0, "押し続けて燃料が尽きた");
    // 燃料がなければ、ボタンを押していても落ちる。
    let before = jet.y16();
    for _ in 0..30 {
        jet.tick(true);
    }
    assert!(jet.y16() > before, "落ちる");
}

/// 接地すると燃料が戻る。ボタンを押さなければ減らない。燃料は最大を超えない。
#[test]
fn fuel_recharges_on_the_ground_and_only_burns_when_held() {
    let def = LevitationDef { fuel_max: 100, thrust: 8, rise_max: 40, recharge: 5 };
    let mut arena = Arena::new(Some(def));
    assert_eq!(arena.fuel(), 100);
    for _ in 0..20 {
        arena.tick(false);
    }
    assert_eq!(arena.fuel(), 100, "押さなければ減らない");
    arena.hold_for(60);
    let spent = arena.fuel();
    assert!(spent < 100);
    // 離して、床に降りるまで待つと、満タンまで戻る。
    for _ in 0..400 {
        arena.tick(false);
    }
    assert!(arena.entities.get(arena.player).unwrap().grounded);
    assert_eq!(arena.fuel(), 100);
}

/// 装備がなければ何も起きない。倒れていれば使えない。値は範囲に収まる。
#[test]
fn no_equipment_or_dead_players_cannot_levitate_and_values_are_clamped() {
    let mut none = Arena::new(None);
    none.hold_for(5);
    assert!(!none.game.step_levitation(&mut none.entities, none.player, true));

    let mut dead = Arena::new(Some(SHORT));
    dead.game.vitals.get_mut(dead.player).unwrap().damage(MAX_HP);
    dead.tick(true); // 地上では何も起きない。
    let top_dead = dead.hold_for(60);
    let mut alive = Arena::new(Some(SHORT));
    let top_alive = alive.hold_for(60);
    assert!(top_dead > top_alive, "倒れていると浮かない");

    let wild = Levitation::new(LevitationDef {
        fuel_max: 10,
        thrust: i16::MAX,
        rise_max: i16::MIN,
        recharge: 0,
    });
    assert_eq!((wild.def.thrust, wild.def.rise_max), (96, 0), "範囲に収める");
}

/// 同じ手順なら、同じ軌跡。
#[test]
fn levitation_is_deterministic() {
    let run = || {
        let mut arena = Arena::new(Some(SHORT));
        (0..300)
            .map(|i| {
                arena.tick(i % 90 < 60);
                (arena.y16(), arena.fuel())
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}
