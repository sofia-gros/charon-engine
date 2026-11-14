//! 仕様 10 §17: 状態異常 (燃える・濡れる・毒)。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::{DefinitionFiles, Definitions};
use charon_game::game::{GameInput, GameState};
use charon_game::items::EquipSlot;
use charon_game::projectile::Payload;
use charon_game::status::{
    BURN_INTERVAL, POISON_INTERVAL, SLOW_TICKS, StatusKind, TICKS_MAX, WET_TICKS,
};
use charon_game::vitals::{DamageKind, MAX_HP, Vitals};
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

const COLD_SCENE_ITEMS: &str = r#"[
    (name: "cold_blade", kind: Weapon(class: Melee, damage: 20, kind: Cold)),
]"#;
const COLD_SCENE_ENEMIES: &str = r#"[
    (name: "dummy", width: 8, height: 16, max_hp: 100, walk_speed: 0,
     jump_speed: 0, sight: 1, attack: None),
    (name: "walker", width: 8, height: 16, max_hp: 100, walk_speed: 15,
     jump_speed: 0, sight: 100, attack: None),
]"#;

fn cold_arena(seed: u64) -> (Arena, Definitions, EntityId) {
    let materials = MaterialTable::builtin().unwrap();
    let mut defs = Definitions::engine_defaults(&materials).unwrap();
    defs.layer(
        DefinitionFiles {
            items: Some(COLD_SCENE_ITEMS),
            enemies: Some(COLD_SCENE_ENEMIES),
            ..Default::default()
        },
        &materials,
        Some("test"),
    )
    .unwrap();
    let mut world = World::new(Arc::new(materials), seed);
    world.insert_chunk(ChunkPos::new(0, 0));
    testkit::fill(&mut world, 0, 52, 63, 52, '#');
    let mut entities = EntityWorld::new();
    let player = entities.spawn_player(10 * 16, 26 * 16, &world).unwrap();
    let mut game = GameState::from_definitions(&defs);
    game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
    game.give_inventory(player).unwrap();
    let target = game.spawn_enemy(&defs, &mut entities, &world, "test:dummy", (30, 26)).unwrap();
    let arena = Arena { world, physics: PhysicsEngine::new(), entities, game, player, tick: 0 };
    (arena, defs, target)
}

fn shoot_cold(arena: &mut Arena) {
    arena
        .game
        .projectiles
        .spawn_kind(
            &arena.world,
            29 * 16,
            32 * 16,
            96,
            0,
            0,
            8,
            20,
            DamageKind::Cold,
            Payload::None,
        )
        .unwrap();
    arena.step();
}

/// 冷気の近接・弾の直撃だけが減速を付け、猶予や耐性で 0 なら付けない。
#[test]
fn cold_hit_applies_slow_once() {
    for seed in 1..=8 {
        let (mut arena, defs, target) = cold_arena(seed);
        let sword = defs.item_id("test:cold_blade", arena.world.materials()).unwrap();
        arena.game.inventories.get_mut(&arena.player).unwrap().add_item(sword, 1);
        arena.game.equip(arena.player, sword, &defs).unwrap();
        assert_eq!(arena.game.equipment[&arena.player].get(EquipSlot::Weapon), Some(sword));
        arena
            .game
            .step(
                &mut arena.world,
                &mut arena.physics,
                &mut arena.entities,
                &[(arena.player, GameInput { fire: true, aim: (1, 0), ..Default::default() })],
                arena.tick,
                seed,
            )
            .unwrap();
        arena.tick += 1;
        assert!(arena.game.vitals.get(target).unwrap().hp < MAX_HP, "seed {seed}: 近接命中");
        assert_eq!(arena.game.statuses.remaining(target, StatusKind::Slow), SLOW_TICKS);
        arena.game.statuses.apply(target, StatusKind::Slow, 20);
        assert_eq!(arena.game.statuses.remaining(target, StatusKind::Slow), SLOW_TICKS);
        let active_hash = arena.game.state_hash();
        arena.game.statuses.clear(target);
        assert_ne!(arena.game.state_hash(), active_hash, "seed {seed}: 減速は同期状態");
        shoot_cold(&mut arena);
        assert!(!arena.has(StatusKind::Slow));
        assert!(
            !arena.game.statuses.has(target, StatusKind::Slow),
            "seed {seed}: 猶予中は付けない"
        );

        let (mut bullet, _, victim) = cold_arena(seed);
        bullet.game.vitals.get_mut(victim).unwrap().armor.cold = 100;
        shoot_cold(&mut bullet);
        assert_eq!(bullet.game.vitals.get(victim).unwrap().hp, MAX_HP);
        assert!(!bullet.game.statuses.has(victim, StatusKind::Slow), "seed {seed}: 100% 耐性");
        bullet.game.vitals.get_mut(victim).unwrap().armor.cold = 0;
        shoot_cold(&mut bullet);
        assert_eq!(bullet.game.statuses.remaining(victim, StatusKind::Slow), SLOW_TICKS);
        assert!(bullet.game.vitals.get(victim).unwrap().hp < MAX_HP, "seed {seed}: 弾の直撃");
    }
}

/// 残り 1 でも最後の移動が減速し、次の移動から元の速さに戻る。
#[test]
fn slow_expires_and_movement_recovers() {
    for seed in 1..=8 {
        let (mut arena, _, _) = cold_arena(seed);
        let id = arena.player;
        assert!(arena.game.statuses.apply(id, StatusKind::Slow, 2));
        let input = [(id, PlayerInput { horizontal: 1, jump: false })];
        let x0 = arena.entities.get(id).unwrap().x16;
        arena.game.step_players(&mut arena.entities, &input, &arena.world).unwrap();
        assert_eq!(arena.entities.get(id).unwrap().x16 - x0, 8);
        arena.step();
        assert_eq!(arena.game.statuses.remaining(id, StatusKind::Slow), 1);
        let x1 = arena.entities.get(id).unwrap().x16;
        arena.game.step_players(&mut arena.entities, &input, &arena.world).unwrap();
        assert_eq!(arena.entities.get(id).unwrap().x16 - x1, 8);
        let bad = (EntityId(u64::MAX), GameInput::default());
        assert!(
            arena
                .game
                .step(
                    &mut arena.world,
                    &mut arena.physics,
                    &mut arena.entities,
                    &[bad],
                    arena.tick,
                    seed
                )
                .is_err()
        );
        assert_eq!(arena.game.statuses.remaining(id, StatusKind::Slow), 1);
        arena.step();
        assert!(!arena.has(StatusKind::Slow));
        let x2 = arena.entities.get(id).unwrap().x16;
        arena.game.step_players(&mut arena.entities, &input, &arena.world).unwrap();
        assert_eq!(arena.entities.get(id).unwrap().x16 - x2, 16, "seed {seed}: 歩幅が戻る");

        let (mut normal, defs, _) = cold_arena(seed);
        let (mut slowed, _, _) = cold_arena(seed);
        let n = normal
            .game
            .spawn_enemy(&defs, &mut normal.entities, &normal.world, "test:walker", (44, 26))
            .unwrap();
        let s = slowed
            .game
            .spawn_enemy(&defs, &mut slowed.entities, &slowed.world, "test:walker", (44, 26))
            .unwrap();
        slowed.game.statuses.apply(s, StatusKind::Slow, 2);
        let nx = normal.entities.get(n).unwrap().x16;
        let sx = slowed.entities.get(s).unwrap().x16;
        normal.step();
        slowed.step();
        assert_eq!(nx - normal.entities.get(n).unwrap().x16, 15, "seed {seed}: 通常の敵");
        assert_eq!(sx - slowed.entities.get(s).unwrap().x16, 7, "seed {seed}: 敵も整数の半分");
    }
}

/// 減速の時計はセルの dirty を立てず、静かな地形は状態が切れる前に眠る。
#[test]
fn status_reaches_sleep_without_writes() {
    for seed in 1..=8 {
        let (mut arena, _, _) = cold_arena(seed);
        testkit::assert_sleeps(&mut arena.world, 100);
        let before = arena.world.state_hash();
        let stone_count = testkit::count(&arena.world, '#');
        arena.game.statuses.apply(arena.player, StatusKind::Slow, SLOW_TICKS);
        for _ in 0..SLOW_TICKS {
            arena.step();
            assert_eq!(arena.world.state_hash(), before, "seed {seed}: セルに書かない");
            assert_eq!(arena.world.awake_chunks(), 0, "seed {seed}: 再起床しない");
        }
        assert_eq!(testkit::count(&arena.world, '#'), stone_count);
        assert!(!arena.has(StatusKind::Slow));

        // 冷えた水は状態異常ではなく、仕様 03 の相変化だけで氷になる。
        let pos = charon_core::coords::CellPos::new(20, 51);
        testkit::place(&mut arena.world, pos.x, pos.y, 'w');
        let mut water = arena.world.get(pos);
        water.temp = -30;
        arena.world.set(pos, water);
        arena.world.step();
        assert_eq!(arena.world.get(pos).material, testkit::material_of(&arena.world, 'i'));
    }
}
