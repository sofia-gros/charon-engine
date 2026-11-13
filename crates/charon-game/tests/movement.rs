//! 仕様 10 §8.4: プレイヤーの移動の操作性 (段差の自動乗り越え・コヨーテタイム・ジャンプの先行入力)。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::{
    COYOTE_TICKS, EntityId, EntityWorld, JUMP_BUFFER_TICKS, PLAYER_HEIGHT, PLAYER_WIDTH,
    PlayerInput, STEP_UP_CELLS,
};

/// 床 (y = 60、石) だけの 64×64 の世界。
fn flat_world() -> World {
    let materials = MaterialTable::builtin().unwrap();
    let mut world = World::new(Arc::new(materials), 1);
    world.insert_chunk(ChunkPos::new(0, 0));
    testkit::fill(&mut world, 0, 60, 63, 60, '#');
    world
}

struct Walker {
    world: World,
    entities: EntityWorld,
    id: EntityId,
}

impl Walker {
    /// 床の上の `x` に、プレイヤーを立たせる (落ちて接地するまで待つ)。
    fn new(world: World, x: i32) -> Self {
        let mut entities = EntityWorld::new();
        let id = entities.spawn_player(i64::from(x) * 16, 30 * 16, &world).unwrap();
        let mut walker = Self { world, entities, id };
        for _ in 0..60 {
            walker.step(0, false);
        }
        assert!(walker.get().grounded, "床に立つ");
        walker
    }

    fn step(&mut self, horizontal: i8, jump: bool) {
        self.entities
            .step_players(&[(self.id, PlayerInput { horizontal, jump })], &self.world)
            .unwrap();
    }

    fn get(&self) -> &charon_game::Entity {
        self.entities.get(self.id).unwrap()
    }

    fn x(&self) -> i64 {
        self.get().x16.div_euclid(16)
    }

    /// 足の下の端のセルの行 (床の上に立っているなら、床の行 − 1)。
    fn feet_row(&self) -> i64 {
        (self.get().y16 + i64::from(PLAYER_HEIGHT) * 16 - 1).div_euclid(16)
    }

    /// 右へ `ticks` tick 歩く。
    fn walk_right(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.step(1, false);
        }
    }
}

/// 床の上の x = 30.. に、高さ `height` の段 (石) を置く。
fn with_step(height: i32) -> World {
    let mut world = flat_world();
    testkit::fill(&mut world, 30, 60 - height, 63, 59, '#');
    world
}

/// `STEP_UP_CELLS` までの段は、歩くだけで乗り越えて、上に立つ。それより高い壁は乗り越えられない。
#[test]
fn small_steps_are_climbed_automatically_and_walls_are_not() {
    for height in 1..=STEP_UP_CELLS as i32 {
        let mut walker = Walker::new(with_step(height), 10);
        walker.walk_right(40);
        assert!(walker.x() >= 32, "高さ {height} の段を越える: x = {}", walker.x());
        assert!(walker.get().grounded, "高さ {height}: 上に立って接地している");
        assert_eq!(walker.feet_row(), i64::from(59 - height), "段の上に、ぴったり立つ");
    }
    for height in [STEP_UP_CELLS as i32 + 1, 16, 20] {
        let mut walker = Walker::new(with_step(height), 10);
        walker.walk_right(60);
        let flush = 30 - i64::from(PLAYER_WIDTH);
        assert!(walker.x() <= flush, "高さ {height} の壁は越えられない: x = {}", walker.x());
        assert_eq!(walker.feet_row(), 59, "床の上のまま");
    }
}

/// 1 セルずつの階段 (斜面) を、立ち止まらずに登り切る。左向きにも降りられる。
#[test]
fn a_staircase_of_single_cell_steps_is_climbed_without_stopping() {
    let mut world = flat_world();
    for i in 0..6 {
        // 幅 2 セルの段が、1 セルずつ高くなる。
        testkit::fill(&mut world, 16 + i * 2, 59 - i, 63, 59, '#');
    }
    let mut walker = Walker::new(world, 2);
    let mut stalled = 0;
    let mut last_x = walker.get().x16;
    for _ in 0..120 {
        walker.step(1, false);
        // 最上段に着くまでの間に、進まなかった tick を数える。
        if walker.get().x16 == last_x && walker.x() < 16 + 5 * 2 {
            stalled += 1;
        }
        last_x = walker.get().x16;
    }
    assert!(walker.x() >= 16 + 5 * 2, "階段の最上段まで: x = {}", walker.x());
    assert!(walker.feet_row() <= 59 - 5, "6 段ぶん高い");
    assert_eq!(stalled, 0, "止まらない");
}

/// 空中では乗り越えない: 接地していない (各 tick の初めに接地を外す) と、高さ 1 の段でも持ち上がらず、壁として止まる。
/// 接地していれば、同じ段を乗り越える。
#[test]
fn steps_are_not_climbed_in_the_air() {
    let flush = 30 - i64::from(PLAYER_WIDTH);
    let mut airborne = Walker::new(with_step(1), 10);
    for _ in 0..60 {
        let id = airborne.id;
        airborne.entities.get_mut(id).unwrap().grounded = false;
        airborne.step(1, false);
        airborne.entities.get_mut(id).unwrap().coyote = 0;
    }
    // 楕円の下は丸いので、壁の手前で止まる位置は、箱がちょうど接する位置より少し (1〜2 セル) 先になる。
    assert!(
        airborne.x() <= flush + 6,
        "接地していないと、1 セルの段でも止まる: x = {}",
        airborne.x()
    );
    assert_eq!(airborne.feet_row(), 59, "持ち上がらない");
    let mut walking = Walker::new(with_step(1), 10);
    walking.walk_right(60);
    assert!(walking.x() > 30, "接地していれば、同じ段を乗り越える: x = {}", walking.x());
    assert_eq!(walking.feet_row(), 58, "段の上に立つ");
}

/// 持ち上げる先に頭が入る空間がなければ、乗り越えない (天井の低いところ)。
#[test]
fn steps_under_a_low_ceiling_are_not_climbed() {
    let mut world = with_step(2);
    // 段の上の、頭の高さ (段の上から 5 セル上) に天井を置く: 高さ 24 のプレイヤーは入れない。
    testkit::fill(&mut world, 30, 40, 63, 52, '#');
    let mut walker = Walker::new(world, 10);
    walker.walk_right(60);
    assert!(walker.x() <= 30 - i64::from(PLAYER_WIDTH), "x = {}", walker.x());
}

/// 足場の端から歩き出ても、`COYOTE_TICKS` の間はジャンプできる。それを過ぎたら、ジャンプできない。
#[test]
fn jumping_works_for_a_few_ticks_after_walking_off_a_ledge() {
    let ledge = || {
        let materials = MaterialTable::builtin().unwrap();
        let mut world = World::new(Arc::new(materials), 1);
        // 下にも十分な高さ (端から落ちて、すぐ底に着地しないように)。
        world.insert_chunk(ChunkPos::new(0, 0));
        world.insert_chunk(ChunkPos::new(0, 1));
        testkit::fill(&mut world, 0, 60, 29, 60, '#'); // x = 29 までの床。その先は何もない。
        world
    };
    // 足場を離れる (接地をやめる) 最初の tick を探す。
    let mut probe = Walker::new(ledge(), 10);
    let mut leave = None;
    for tick in 0..200 {
        probe.step(1, false);
        if !probe.get().grounded {
            leave = Some(tick);
            break;
        }
    }
    let leave = leave.expect("端から歩き出す");
    for after in 0..=(COYOTE_TICKS as usize + 4) {
        let mut walker = Walker::new(ledge(), 10);
        for _ in 0..=leave {
            walker.step(1, false);
        }
        assert!(!walker.get().grounded);
        // 離れてから `after` tick 目に、ジャンプを押す。
        for _ in 0..after {
            walker.step(1, false);
        }
        let before_vy = walker.get().vy;
        walker.step(1, true);
        let jumped = walker.get().vy < before_vy && walker.get().vy < 0;
        // 接地をやめた tick を 0 として、`after + 1` tick 後に押した。猶予は、接地をやめた tick を入れて
        // `COYOTE_TICKS` 回の tick (0..COYOTE_TICKS)。
        assert_eq!(
            jumped,
            after + 1 < COYOTE_TICKS as usize,
            "離れてから {} tick 後 (猶予 {COYOTE_TICKS} tick)",
            after + 1
        );
    }
}

/// 跳んだ直後は、コヨーテタイムで二段ジャンプできない。
#[test]
fn coyote_time_does_not_allow_a_double_jump() {
    let mut walker = Walker::new(flat_world(), 10);
    walker.step(0, true);
    assert!(walker.get().vy < 0, "跳ぶ");
    assert_eq!(walker.get().coyote, 0);
    let after_first = walker.get().y16;
    for _ in 0..4 {
        walker.step(0, true);
    }
    // 押し続けても、空中では二度目の上向きの初速はつかない (上がり続けるが、速さは減っていく)。
    assert!(walker.get().vy > -60, "二段ジャンプはない: vy = {}", walker.get().vy);
    assert!(walker.get().y16 < after_first, "上がっている");
}

/// 着地の `JUMP_BUFFER_TICKS` 未満前に押したジャンプは、着地したら跳ぶ。それより前に押しても跳ばない。
#[test]
fn a_jump_pressed_just_before_landing_fires_on_landing() {
    // 高いところから落として、最初に接地する tick を調べる。
    let drop = || {
        let world = flat_world();
        let mut entities = EntityWorld::new();
        let id = entities.spawn_player(10 * 16, 2 * 16, &world).unwrap();
        Walker { world, entities, id }
    };
    let mut probe = drop();
    let mut landing = 0;
    for tick in 0..200 {
        probe.step(0, false);
        if probe.get().grounded {
            landing = tick;
            break;
        }
    }
    assert!(landing > 10);
    for before in 0..=(JUMP_BUFFER_TICKS as usize + 4) {
        let mut walker = drop();
        let press = landing - before;
        let mut jumped = false;
        for tick in 0..landing + 6 {
            walker.step(0, tick == press);
            if tick > landing && walker.get().vy < 0 {
                jumped = true;
            }
        }
        assert_eq!(
            jumped,
            before < JUMP_BUFFER_TICKS as usize,
            "着地の {before} tick 前に押した (先行入力 {JUMP_BUFFER_TICKS} tick)"
        );
    }
}

/// 体は 2×3 ブロック (16×24 セル)。ジャンプは 3〜4 ブロック (24..=32 セル) の高さ。
#[test]
fn the_body_is_two_by_three_blocks_and_jumps_three_to_four_blocks() {
    assert_eq!((PLAYER_WIDTH, PLAYER_HEIGHT), (16, 24));
    let mut walker = Walker::new(flat_world(), 10);
    let floor = walker.get().y16;
    let mut top = floor;
    for tick in 0..120 {
        walker.step(0, tick == 0);
        top = top.min(walker.get().y16);
    }
    let height = (floor - top) / 16;
    assert!((24..=32).contains(&height), "ジャンプの高さ {height} セル");
}

/// 新しい状態 (`coyote`・`jump_buffer`) も状態のハッシュに入る (同期の検査で、食い違いを見つけられる)。
#[test]
fn the_new_movement_state_is_part_of_the_state_hash() {
    let world = flat_world();
    let mut a = EntityWorld::new();
    let mut b = EntityWorld::new();
    let ida = a.spawn_player(10 * 16, 30 * 16, &world).unwrap();
    let idb = b.spawn_player(10 * 16, 30 * 16, &world).unwrap();
    assert_eq!(a.state_hash(), b.state_hash());
    a.get_mut(ida).unwrap().coyote = 3;
    assert_ne!(a.state_hash(), b.state_hash());
    b.get_mut(idb).unwrap().coyote = 3;
    assert_eq!(a.state_hash(), b.state_hash());
    b.get_mut(idb).unwrap().jump_buffer = 1;
    assert_ne!(a.state_hash(), b.state_hash());
}

/// 同じ入力なら、同じ軌跡 (段差・ジャンプを含めて)。
#[test]
fn movement_with_steps_is_deterministic() {
    let run = || {
        let mut world = flat_world();
        for i in 0..6 {
            testkit::fill(&mut world, 20 + i * 4, 59 - i, 23 + i * 4, 59, '#');
        }
        let mut walker = Walker::new(world, 5);
        (0..300)
            .map(|i| {
                walker.step(if i % 70 < 55 { 1 } else { -1 }, i % 37 == 0);
                (walker.get().x16, walker.get().y16, walker.get().coyote, walker.get().jump_buffer)
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}
