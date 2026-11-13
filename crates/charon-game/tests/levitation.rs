//! 仕様 10 §18: 浮遊 (長押しでホバー、短押しはふつうのジャンプ)。

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

/// 燃料が 1 秒ぶん、0.2 秒 (12 tick) 押し続けるとホバーが始まる浮遊。
const SHORT: LevitationDef =
    LevitationDef { fuel_max: 60, delay: 12, thrust: 8, rise_max: 6, recharge: 3 };

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

    fn grounded(&self) -> bool {
        self.entities.get(self.player).unwrap().grounded
    }

    fn fuel(&self) -> Option<u16> {
        self.game.levitation.get(&self.player).map(|l| l.fuel)
    }

    /// ボタンを押して、`pressed` tick 押し続けてから離す。最初に着地するまでの tick (空中にいた長さ) と、
    /// いちばん高くなった位置 (y16 が小さい方) を返す。着地しなければ上限の 600 tick。
    fn jump(&mut self, pressed: usize) -> (usize, i64) {
        let mut top = self.y16();
        let mut air = 0;
        let mut left_ground = false;
        for i in 0..600 {
            self.tick(i < pressed);
            top = top.min(self.y16());
            if !self.grounded() {
                left_ground = true;
                air += 1;
            } else if left_ground {
                break;
            }
        }
        (air, top)
    }
}

/// 短く押す (`delay` より短い) だけなら、浮遊の装備があっても、ふつうのジャンプと**まったく同じ**軌跡で、燃料も減らない。
#[test]
fn a_short_press_is_a_plain_jump() {
    let trace = |def: Option<LevitationDef>, pressed: usize| {
        let mut arena = Arena::new(def);
        let ys: Vec<i64> = (0..200)
            .map(|i| {
                arena.tick(i < pressed);
                arena.y16()
            })
            .collect();
        (ys, arena.fuel())
    };
    for pressed in [1, 5, 12] {
        let (plain, _) = trace(None, pressed);
        let (equipped, fuel) = trace(Some(SHORT), pressed);
        assert_eq!(plain, equipped, "{pressed} tick 押しただけ: ふつうのジャンプと同じ");
        assert_eq!(fuel, Some(60), "燃料は減らない");
    }
}

/// 長押しすると、ジャンプの頂点で落ちずにとどまり (ゆっくり上がり)、燃料が尽きると落ちる。
#[test]
fn holding_past_the_delay_hovers_until_the_fuel_runs_out() {
    let (plain_air, plain_top) = Arena::new(None).jump(5);
    let mut hover = Arena::new(Some(SHORT));
    let (hover_air, hover_top) = hover.jump(500);
    assert!(hover_air > plain_air + 40, "空中にいる長さ: ふつう {plain_air}、ホバー {hover_air}");
    assert!(hover_top < plain_top, "頂点より高く上がる: ふつう {plain_top}、ホバー {hover_top}");
    // 燃料は、ホバーした分だけ減って、着地で少し戻る (最大 60 を超えない)。
    assert!(hover.fuel().unwrap() <= 60);
    // 燃料が尽きたら、押し続けていても落ちる。
    let mut empty = Arena::new(Some(SHORT));
    let mut ticks = 0;
    while empty.fuel() != Some(0) && ticks < 500 {
        empty.tick(true);
        ticks += 1;
    }
    assert_eq!(empty.fuel(), Some(0), "押し続けて燃料が尽きた ({ticks} tick)");
    let at_empty = empty.y16();
    for _ in 0..30 {
        empty.tick(true);
    }
    assert!(empty.y16() > at_empty, "燃料がなければ、押していても落ちる");
}

/// ホバー中に押している間だけ燃料が減る。ボタンを離すと落ち始める。接地すると燃料が戻る。
#[test]
fn fuel_burns_only_while_hovering_and_recharges_on_the_ground() {
    let def = LevitationDef { fuel_max: 100, delay: 12, thrust: 8, rise_max: 6, recharge: 5 };
    let mut arena = Arena::new(Some(def));
    assert_eq!(arena.fuel(), Some(100));
    for _ in 0..20 {
        arena.tick(false);
    }
    assert_eq!(arena.fuel(), Some(100), "押さなければ減らない");
    // 押し続ける: ジャンプの最中 (速く上がっている間) は減らず、頂点に近づいてから減る。
    for _ in 0..10 {
        arena.tick(true);
    }
    assert_eq!(arena.fuel(), Some(100), "ジャンプの最中は減らない");
    for _ in 0..60 {
        arena.tick(true);
    }
    let spent = arena.fuel().unwrap();
    assert!(spent < 100, "ホバーで減る: {spent}");
    // 離すと、減らなくなって落ちる。床に降りると満タンまで戻る。
    for _ in 0..600 {
        arena.tick(false);
    }
    assert!(arena.grounded());
    assert_eq!(arena.fuel(), Some(100));
}

/// 装備がなければ何も起きない。倒れていれば使えない。値は範囲に収まる。押し続けた tick を数える。
#[test]
fn no_equipment_or_dead_players_cannot_levitate_and_values_are_clamped() {
    let mut none = Arena::new(None);
    for _ in 0..40 {
        none.tick(true);
    }
    assert!(!none.game.step_levitation(&mut none.entities, none.player, true));

    let mut dead = Arena::new(Some(SHORT));
    dead.game.vitals.get_mut(dead.player).unwrap().damage(MAX_HP);
    let (_, top_dead) = dead.jump(500);
    let (_, top_alive) = Arena::new(Some(SHORT)).jump(500);
    assert!(top_dead > top_alive, "倒れていると浮かない");

    let wild = Levitation::new(LevitationDef {
        fuel_max: 10,
        delay: 0,
        thrust: i16::MAX,
        rise_max: i16::MIN,
        recharge: 0,
    });
    assert_eq!((wild.def.thrust, wild.def.rise_max), (96, 0), "範囲に収める");

    let mut counted = Arena::new(Some(SHORT));
    for _ in 0..5 {
        counted.tick(true);
    }
    assert_eq!(counted.game.levitation[&counted.player].held_ticks(), 5);
    counted.tick(false);
    assert_eq!(counted.game.levitation[&counted.player].held_ticks(), 0, "離すと 0 にもどる");
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
