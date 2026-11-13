#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::{DefinitionFiles, Definitions};
use charon_game::game::{GameError, GameInput, GameState, StepReport};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{Entity, EntityId, EntityWorld, PlayerInput, step_entity};
use charon_physics::PhysicsEngine;
use charon_physics::rope::RopePhase;

/// 追加のフック。`swing` は巻き取りも引っ張りもしない (振り子だけ)、`short` は最大の長さが短い。
const TEST_HOOKS: &str = r#"[
    (name: "swing", max_length: 100, min_length: 4, speed: 320, segments: 24, gravity: 6,
     iterations: 8, reel_speed: 0, pull_accel: 0, pull_max: 48, cooldown: 20),
    (name: "short", max_length: 10, min_length: 4, speed: 48, segments: 12, gravity: 6,
     iterations: 8, reel_speed: 24, pull_accel: 8, pull_max: 48, cooldown: 20),
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
    /// 幅 64・高さ 61 の世界。床は y = 60。`ceiling` が `Some(y)` なら、その行に天井を張る。
    fn new(seed: u64, ceiling: Option<i32>) -> Self {
        let materials = MaterialTable::builtin().expect("組み込み材質を読み込む");
        let mut defs = Definitions::engine_defaults(&materials).unwrap();
        defs.layer(
            DefinitionFiles { hooks: Some(TEST_HOOKS), ..Default::default() },
            &materials,
            None,
        )
        .unwrap();
        let mut world = World::new(Arc::new(materials), seed);
        world.insert_chunk(ChunkPos::new(0, 0));
        testkit::fill(&mut world, 0, 60, 63, 60, '#');
        if let Some(y) = ceiling {
            testkit::fill(&mut world, 0, y, 63, y, '#');
        }
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

    /// 体力・杖・フック (`hook`) を持つプレイヤーを `(x, y)` (左上のセル) に置く。
    fn add_player(&mut self, x: i64, y: i64, hook: Option<&str>) -> EntityId {
        let id = self.entities.spawn_player(x * 16, y * 16, &self.world).unwrap();
        self.game.add_character(id, Vitals::new(MAX_HP).unwrap(), self.defs.wand("basic").unwrap());
        if let Some(name) = hook {
            self.game.equip_hook(id, &self.defs, name).unwrap();
        }
        id
    }

    /// プレイヤーの移動 → ゲーム状態 (Sim と同じ順序)。
    fn step(&mut self, id: EntityId, walk: i8, input: GameInput) -> StepReport {
        self.entities
            .step_players(&[(id, PlayerInput { horizontal: walk, jump: false })], &self.world)
            .unwrap();
        let seed = self.world.seed();
        let report = self
            .game
            .step(
                &mut self.world,
                &mut self.physics,
                &mut self.entities,
                &[(id, input)],
                self.tick,
                seed,
            )
            .unwrap();
        self.tick += 1;
        report
    }

    fn entity(&self, id: EntityId) -> &Entity {
        self.entities.get(id).unwrap()
    }

    fn hand(&self, id: EntityId) -> [f32; 2] {
        let e = self.entity(id);
        [
            (e.x16 + i64::from(e.width) * 8) as f32 / 16.0,
            (e.y16 + i64::from(e.height) * 8) as f32 / 16.0,
        ]
    }
}

fn tool(aim: (i32, i32)) -> GameInput {
    GameInput { tool: true, aim, ..Default::default() }
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

#[test]
fn hook_launches_and_anchors() {
    for seed in 1..=8 {
        let mut arena = Arena::new(seed, None);
        testkit::fill(&mut arena.world, 50, 0, 51, 59, '#');
        let player = arena.add_player(20, 44, Some("grapple"));
        // 狙いが (0, 0) なら撃たない。
        arena.step(player, 0, tool((0, 0)));
        assert!(arena.game.rope(player).is_none(), "seed={seed}");
        arena.step(player, 0, GameInput::default());

        // 右へ撃つ。1 tick 目は飛行中で、数 tick で壁に着く。
        arena.step(player, 0, tool((1, 0)));
        let rope = arena.game.rope(player).expect("押した tick に出る");
        assert_eq!(rope.phase(), RopePhase::Flying);
        let mut anchored = None;
        for tick in 0..6 {
            arena.step(player, 0, tool((1, 0)));
            if let Some(anchor) = arena.game.rope(player).and_then(|r| r.anchor()) {
                anchored = Some((tick, anchor));
                break;
            }
        }
        let (tick, anchor) = anchored.expect("壁に着く");
        assert!(tick <= 3, "seed={seed}: {tick} tick かかった");
        assert!((49.0..50.0).contains(&anchor[0]), "壁の手前で止まる: {anchor:?}");
        assert!(anchor[1] > 44.0 && anchor[1] < 60.0);
    }
}

#[test]
fn hook_pulls_player_toward_anchor() {
    for seed in 1..=8 {
        // 天井 (y = 5) の下で、真上のやや右へ撃つ。
        let mut arena = Arena::new(seed, Some(5));
        let player = arena.add_player(20, 44, Some("grapple"));
        for _ in 0..2 {
            arena.step(player, 0, GameInput::default());
        }
        let start = arena.hand(player);
        let mut anchor = None;
        for _ in 0..4 {
            arena.step(player, 0, tool((1, -2)));
            anchor = anchor.or_else(|| arena.game.rope(player).and_then(|r| r.anchor()));
        }
        let anchor = anchor.expect("天井に着く");
        assert!(anchor[1] < 8.0, "天井の近く: {anchor:?}");
        let first = distance(start, anchor);
        let mut closest = first;
        for _ in 0..240 {
            arena.step(player, 0, tool((1, -2)));
            closest = closest.min(distance(arena.hand(player), anchor));
            // 天井や床を抜けない。
            let e = arena.entity(player);
            assert!(
                e.y16 >= 6 * 16 && e.y16 + i64::from(e.height) * 16 <= 60 * 16,
                "seed={seed}: y16={}",
                e.y16
            );
        }
        let end = distance(arena.hand(player), anchor);
        assert!(first > 30.0 && end < first - 20.0, "seed={seed}: {first} → {end}");
        assert!(closest >= 3.0, "天井にめり込まない: {closest}");
        // 天井の下では箱の高さ (16 セル) の半分より近づけない。
        assert!(end < first / 2.0, "アンカーの近くまで引かれる: {end}");
    }
}

#[test]
fn hook_swings_without_stretching() {
    for seed in 1..=8 {
        let mut arena = Arena::new(seed, Some(5));
        let player = arena.add_player(30, 20, Some("swing"));
        arena.step(player, 0, tool((0, -1)));
        let mut rest = None;
        let mut xs = Vec::new();
        for tick in 0..400 {
            // 最初の 20 tick は右へ歩いて勢いをつけ、あとは離して揺らす。
            let walk = i8::from(tick < 20);
            arena.step(player, walk, tool((0, -1)));
            let rope = arena.game.rope(player).expect("つながったまま");
            if rope.phase() == RopePhase::Anchored {
                let r = *rest.get_or_insert(rope.rest_length());
                let anchor = rope.anchor().unwrap();
                let stretch = distance(arena.hand(player), anchor) - r;
                assert!(stretch < 2.5, "seed={seed} tick {tick}: 伸びすぎ {stretch}");
                assert_eq!(rope.rest_length(), r, "swing は巻き取らない");
            }
            xs.push(arena.hand(player)[0]);
        }
        let (min, max) =
            xs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
        assert!(max - min > 10.0, "seed={seed}: 振り子のように揺れる {min}..{max}");
        assert!(min < 30.0 + 2.0 && max > 30.0, "アンカーの両側へ");
        let e = arena.entity(player);
        assert!(e.y16 + i64::from(e.height) * 16 <= 60 * 16, "床を抜けない");
    }
}

#[test]
fn hook_release_and_cooldown() {
    let mut arena = Arena::new(1, None);
    testkit::fill(&mut arena.world, 50, 0, 51, 59, '#');
    let player = arena.add_player(20, 44, Some("grapple"));
    for _ in 0..6 {
        arena.step(player, 0, tool((1, 0)));
    }
    assert!(arena.game.rope(player).and_then(|r| r.anchor()).is_some());
    // 離すと外れ、待ち時間 (20 tick) のあいだは撃てない。
    arena.step(player, 0, GameInput::default());
    assert!(arena.game.rope(player).is_none());
    let cooldown = arena.game.hooks[&player].cooldown();
    assert_eq!(cooldown, 20);
    for _ in 0..10 {
        arena.step(player, 0, tool((1, 0)));
        assert!(arena.game.rope(player).is_none(), "待ち時間のあいだは撃てない");
        arena.step(player, 0, GameInput::default());
    }
    // 押し直した tick に、待ち時間が明けていれば撃てる。
    for _ in 0..20 {
        arena.step(player, 0, GameInput::default());
    }
    arena.step(player, 0, tool((1, 0)));
    assert!(arena.game.rope(player).is_some());

    // 倒れると外れる。
    arena.game.vitals.get_mut(player).unwrap().damage(1_000);
    arena.step(player, 0, tool((1, 0)));
    assert!(arena.game.rope(player).is_none(), "倒れたら外れる");
    arena.step(player, 0, tool((1, 0)));
    assert!(arena.game.rope(player).is_none(), "倒れたあとは撃てない");
}

#[test]
fn hook_respects_max_length() {
    for seed in 1..=8 {
        let mut arena = Arena::new(seed, None);
        // 壁は 30 セル先。`short` の最大の長さは 10 セル。
        testkit::fill(&mut arena.world, 50, 0, 51, 59, '#');
        let player = arena.add_player(20, 44, Some("short"));
        let mut was_out = false;
        for _ in 0..10 {
            arena.step(player, 0, tool((1, 0)));
            if let Some(rope) = arena.game.rope(player) {
                was_out = true;
                assert_eq!(rope.phase(), RopePhase::Flying, "届かないので着かない seed={seed}");
            }
        }
        assert!(was_out, "いったんは出る");
        assert!(arena.game.rope(player).is_none(), "最大の長さを超えたら外れる seed={seed}");
        assert!(arena.game.hooks[&player].cooldown() > 0 || arena.game.rope(player).is_none());
    }
}

#[test]
fn weapon_and_tool_are_independent() {
    let mut arena = Arena::new(1, None);
    testkit::fill(&mut arena.world, 50, 0, 51, 59, '#');
    let hooked = arena.add_player(10, 44, Some("grapple"));
    let plain = arena.add_player(30, 44, None);
    let both = GameInput { fire: true, tool: true, dig: false, aim: (1, 0) };
    let report = {
        arena.entities.step_players(&[], &arena.world).unwrap();
        let seed = arena.world.seed();
        arena
            .game
            .step(
                &mut arena.world,
                &mut arena.physics,
                &mut arena.entities,
                &[(hooked, both), (plain, both)],
                0,
                seed,
            )
            .unwrap()
    };
    assert!(report.hits.is_empty());
    // 同じ tick に、杖の弾 (2 人分) とワイヤー (フックのある 1 人分) が出る。
    assert_eq!(arena.game.projectiles.len(), 2, "武器は 2 人とも撃てる");
    assert!(arena.game.rope(hooked).is_some());
    assert!(arena.game.rope(plain).is_none(), "フックのないキャラクターの tool は何も起こさない");

    // ワイヤーが出ていても、杖は撃てる。杖だけ止めても、ワイヤーは出たまま。
    let mana = arena.game.wands[&hooked].mana;
    for _ in 0..8 {
        arena.step(hooked, 0, GameInput { tool: true, aim: (1, 0), ..Default::default() });
    }
    assert!(arena.game.rope(hooked).is_some());
    assert_eq!(arena.game.wands[&hooked].mana, (mana + 8).min(100), "武器を使っていない");
    arena.step(hooked, 0, both);
    assert!(arena.game.rope(hooked).is_some(), "武器を撃ってもワイヤーは外れない");
    assert!(arena.game.wands[&hooked].mana < 100 || arena.game.projectiles.len() > 2);

    // フックは杖を持つキャラクターにだけ持たせられる。失敗しても何も変えない。
    let ghost = arena.entities.spawn(0, 0, 8, 16).unwrap();
    let before = arena.game.state_hash();
    assert_eq!(
        arena.game.equip_hook(ghost, &arena.defs, "grapple").unwrap_err(),
        GameError::InvalidAction
    );
    assert_eq!(
        arena.game.equip_hook(hooked, &arena.defs, "nothing").unwrap_err(),
        GameError::UnknownHook("nothing".to_owned())
    );
    assert_eq!(arena.game.state_hash(), before);
}

#[test]
fn entity_external_velocity_decays() {
    let materials = MaterialTable::builtin().unwrap();
    let mut world = World::new(Arc::new(materials), 1);
    world.insert_chunk(ChunkPos::new(0, 0));
    testkit::fill(&mut world, 0, 60, 63, 60, '#');
    let neutral = PlayerInput::default();

    // 接地中は 3/4 ずつ減る。
    let mut entities = EntityWorld::new();
    let id = entities.spawn_player(10 * 16, 50 * 16, &world).unwrap();
    for _ in 0..3 {
        step_entity(entities.get_mut(id).unwrap(), neutral, 32, 96, &world);
    }
    assert!(entities.get(id).unwrap().grounded);
    entities.get_mut(id).unwrap().ext_vx = 160;
    let x0 = entities.get(id).unwrap().x16;
    let mut seen = Vec::new();
    for _ in 0..3 {
        step_entity(entities.get_mut(id).unwrap(), neutral, 32, 96, &world);
        seen.push(entities.get(id).unwrap().ext_vx);
    }
    assert_eq!(seen, vec![120, 90, 67]);
    assert_eq!(entities.get(id).unwrap().x16 - x0, 160 + 120 + 90, "外力の速度ぶん動く");

    // 空中は 15/16 ずつ減る。
    let mut entities = EntityWorld::new();
    let id = entities.spawn_player(10 * 16, 5 * 16, &world).unwrap();
    entities.get_mut(id).unwrap().ext_vx = 160;
    step_entity(entities.get_mut(id).unwrap(), neutral, 32, 96, &world);
    assert!(!entities.get(id).unwrap().grounded);
    assert_eq!(entities.get(id).unwrap().ext_vx, 150);
    step_entity(entities.get_mut(id).unwrap(), neutral, 32, 96, &world);
    assert_eq!(entities.get(id).unwrap().ext_vx, 140);

    // 壁に当たると 0 になる。
    let mut entities = EntityWorld::new();
    testkit::fill(&mut world, 20, 0, 20, 59, '#');
    let id = entities.spawn_player(10 * 16, 50 * 16, &world).unwrap();
    entities.get_mut(id).unwrap().ext_vx = 400;
    for _ in 0..30 {
        step_entity(entities.get_mut(id).unwrap(), neutral, 32, 96, &world);
    }
    assert_eq!(entities.get(id).unwrap().ext_vx, 0);
    let width = i64::from(entities.get(id).unwrap().width);
    assert!(entities.get(id).unwrap().x16 + width * 16 <= 20 * 16, "壁を抜けない");

    // ext_vx が違うと状態ハッシュも違う。
    let mut a = EntityWorld::new();
    let mut b = EntityWorld::new();
    let ia = a.spawn(0, 0, 8, 16).unwrap();
    b.spawn(0, 0, 8, 16).unwrap();
    assert_eq!(a.state_hash(), b.state_hash());
    a.get_mut(ia).unwrap().ext_vx = 1;
    assert_ne!(a.state_hash(), b.state_hash());
}

fn trace(seed: u64) -> Vec<u64> {
    let mut arena = Arena::new(seed, Some(5));
    let player = arena.add_player(30, 20, Some("grapple"));
    let mut hashes = Vec::new();
    for tick in 0..300 {
        let tool_down = (tick / 25) % 3 != 2;
        let walk = [1, 0, -1][(tick / 40) % 3];
        let aim = (3 - (tick as i32 / 30) % 7, -2);
        arena.step(player, walk, GameInput { tool: tool_down, aim, ..Default::default() });
        hashes.push(arena.game.state_hash() ^ arena.entities.state_hash().rotate_left(9));
    }
    hashes
}

#[test]
fn hook_is_deterministic() {
    for seed in 1..=8 {
        let (a, b) = (trace(seed), trace(seed));
        assert_eq!(a, b, "seed={seed}");
        let distinct: std::collections::BTreeSet<_> = a.iter().collect();
        assert!(distinct.len() > 150, "状態が動いている: {}", distinct.len());
    }
}
