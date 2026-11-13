//! 仕様 10 §12: 掘る道具 (硬さ・力・円・掘る時間)。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::cell::{FLAG_BODY, MaterialId};
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::{DefinitionFiles, Definitions};
use charon_game::game::{GameError, GameInput, GameState, StepReport};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld, PlayerInput};
use charon_physics::PhysicsEngine;

const DIGS: &str = r#"[
    (name: "weak", power: 1, radius: 3, range: 40, recover: 1),
    (name: "strong", power: 100, radius: 3, range: 40, recover: 1),
]"#;

struct Arena {
    world: World,
    physics: PhysicsEngine,
    entities: EntityWorld,
    game: GameState,
    defs: Definitions,
    player: EntityId,
    tick: u64,
}

impl Arena {
    /// 幅 64・高さ 64 の世界。床は y = 60。プレイヤーは (10, 50) あたりに立ち、右 (x >= 30) に壁 (`ch`) を置く。
    fn new(seed: u64, wall: char, dig: &str) -> Self {
        let materials = MaterialTable::builtin().unwrap();
        let mut defs = Definitions::engine_defaults(&materials).unwrap();
        defs.layer(DefinitionFiles { digs: Some(DIGS), ..Default::default() }, &materials, None)
            .unwrap();
        let mut world = World::new(Arc::new(materials), seed);
        world.insert_chunk(ChunkPos::new(0, 0));
        testkit::fill(&mut world, 0, 60, 63, 60, '#');
        testkit::fill(&mut world, 30, 40, 40, 59, wall);
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(10 * 16, 50 * 16, &world).unwrap();
        let mut game = GameState::from_definitions(&defs);
        game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
        game.equip_dig(player, &defs, dig).unwrap();
        let mut arena =
            Self { world, physics: PhysicsEngine::new(), entities, game, defs, player, tick: 0 };
        // 床に着くまで待つ。
        for _ in 0..30 {
            arena.step(GameInput::default());
        }
        arena
    }

    fn step(&mut self, input: GameInput) -> StepReport {
        self.entities
            .step_players(&[(self.player, PlayerInput { horizontal: 0, jump: false })], &self.world)
            .unwrap();
        let seed = self.world.seed();
        let report = self
            .game
            .step(
                &mut self.world,
                &mut self.physics,
                &mut self.entities,
                &[(self.player, input)],
                self.tick,
                seed,
            )
            .unwrap();
        self.tick += 1;
        report
    }

    /// 右へ向けて掘る。
    fn dig(&mut self) -> StepReport {
        self.step(GameInput { dig: true, aim: (1000, 0), ..Default::default() })
    }

    fn solid_at(&self, x: i32, y: i32) -> bool {
        self.world.get(CellPos::new(x, y)).material != MaterialId::EMPTY
    }

    /// 手と同じ高さ (プレイヤーの中心の行) の壁の最初のセル。
    fn row(&self) -> i32 {
        let e = self.entities.get(self.player).unwrap();
        ((e.y16 + i64::from(e.height) * 8) / 16) as i32
    }
}

/// 掘る時間は `硬さ / 力` tick: 石 (硬さ 600) を力 6 で掘ると 100 tick かかり、その前には 1 セルも消えない。
#[test]
fn dig_removes_cells_after_hardness_over_power_ticks() {
    for seed in 1..=4 {
        let mut arena = Arena::new(seed, '#', "laser");
        let row = arena.row();
        let stone = arena.world.materials().get(arena.world.get(CellPos::new(30, row)).material);
        assert_eq!(stone.hardness, 600, "石の硬さ");
        let mut removed_at = None;
        for tick in 1..=200 {
            let report = arena.dig();
            if !report.dug.is_empty() && removed_at.is_none() {
                removed_at = Some(tick);
            }
        }
        // 力 6 で硬さ 600 は 100 tick。
        assert_eq!(removed_at, Some(100), "seed {seed}");
        assert!(!arena.solid_at(30, row), "seed {seed}: 手の前の壁が消える");
    }
}

/// 硬さの違う材質は、掘る時間が違う: 砂 (硬さ 20) は一瞬、石は長い。力が強い道具は短い。
#[test]
fn dig_time_depends_on_hardness_and_power() {
    let time_to_first_dug = |wall: char, tool: &str| {
        let mut arena = Arena::new(1, wall, tool);
        for tick in 1..=1500 {
            if !arena.dig().dug.is_empty() {
                return tick;
            }
        }
        panic!("掘れない: {wall} {tool}");
    };
    let sand_laser = time_to_first_dug('s', "laser");
    let stone_laser = time_to_first_dug('#', "laser");
    let stone_strong = time_to_first_dug('#', "strong");
    let stone_weak = time_to_first_dug('#', "weak");
    assert_eq!(sand_laser, 4, "砂: 20 / 6 = 4 tick (切り上げ)");
    assert_eq!(stone_laser, 100);
    assert_eq!(stone_strong, 6, "力 100 なら 600 / 100 = 6 tick");
    assert_eq!(stone_weak, 600, "力 1 なら 600 tick");
    assert!(sand_laser < stone_strong && stone_strong < stone_laser && stone_laser < stone_weak);
}

/// 削るのは、当たった点を中心にした**円**。壁の中の円のセルが全部消え、円の外は残る。
#[test]
fn dig_cuts_a_circle() {
    let mut arena = Arena::new(1, '#', "strong");
    let row = arena.row();
    for _ in 0..10 {
        arena.dig();
    }
    // 当たった点は壁の最初のセル (30, row)。半径 3。
    let r = 3;
    let mut inside = 0;
    for dy in -r..=r {
        for dx in -r..=r {
            let (x, y) = (30 + dx, row + dy);
            let in_wall = x >= 30;
            if dx * dx + dy * dy <= r * r {
                if in_wall {
                    assert!(!arena.solid_at(x, y), "円の中は消える ({x}, {y})");
                    inside += 1;
                }
            } else if in_wall {
                assert!(arena.solid_at(x, y), "円の外は残る ({x}, {y})");
            }
        }
    }
    assert!(inside > 15, "円の半分 ({inside})");
}

/// 液体は掘れず、光線は素通りする。剛体・`boundary` は掘れない。
#[test]
fn dig_ignores_liquids_bodies_and_boundary() {
    let mut arena = Arena::new(1, '#', "strong");
    let row = arena.row();
    // 手と壁の間に水と剛体のセル。
    testkit::fill(&mut arena.world, 20, row - 1, 22, row + 1, 'w');
    let water = arena.world.get(CellPos::new(21, row)).material;
    let body_pos = CellPos::new(25, row);
    let body = arena.world.get(body_pos).with(FLAG_BODY, true);
    arena.world.set(body_pos, body);
    for _ in 0..3 {
        arena.dig();
    }
    assert_eq!(arena.world.get(CellPos::new(21, row)).material, water, "水は掘れない");
    assert!(arena.world.get(body_pos).has(FLAG_BODY), "剛体は掘れない");
    // 剛体で光線が止まるので、奥の壁は掘れていない。
    assert!(arena.solid_at(30, row), "剛体の奥は届かない");
    // 剛体をなくすと、水を素通りして壁を掘る。
    let empty = arena.world.make_cell(MaterialId::EMPTY, body_pos);
    arena.world.set(body_pos, empty);
    for _ in 0..8 {
        arena.dig();
    }
    assert!(!arena.solid_at(30, row), "水を素通りして壁を掘る");
    assert_eq!(arena.world.get(CellPos::new(21, row)).material, water);
}

/// 当てるのをやめると、ダメージは回復する。回復する前にまた当てれば、続きから。
#[test]
fn dig_damage_recovers_when_released() {
    let mut arena = Arena::new(1, '#', "laser");
    let row = arena.row();
    let pos = CellPos::new(30, row);
    for _ in 0..40 {
        arena.dig();
    }
    let before = arena.game.digs.get(&arena.player).unwrap().damage_at(pos);
    assert_eq!(before, 40 * 6, "40 tick で 240");
    for _ in 0..30 {
        arena.step(GameInput::default());
    }
    let after = arena.game.digs.get(&arena.player).unwrap().damage_at(pos);
    assert_eq!(after, before - 30 * 2, "1 tick に 2 (recover) 回復する");
    // 十分待つとダメージは消える (表に残らない)。
    for _ in 0..200 {
        arena.step(GameInput::default());
    }
    assert_eq!(arena.game.digs.get(&arena.player).unwrap().damaged_cells(), 0);
    assert!(arena.game.dig_beam(arena.player).is_none(), "押していなければ光線は出ない");
}

/// 掘ったセルは `StepReport::dug` に材質つきで出る (集めるのはゲームの仕事)。セルの数は、消えた分だけ減る。
#[test]
fn dig_reports_dug_cells() {
    let mut arena = Arena::new(1, '#', "strong");
    let before = testkit::count(&arena.world, '#');
    let mut dug = 0;
    let stone = testkit::material_of(&arena.world, '#');
    for _ in 0..10 {
        let report = arena.dig();
        for cell in &report.dug {
            assert_eq!(cell.material, stone);
        }
        dug += report.dug.len();
    }
    assert!(dug > 15);
    assert_eq!(testkit::count(&arena.world, '#'), before - dug, "報告した数だけ消える");
}

/// 光線の向き: 狙いの向きに進み、手から当たった点まで。`dig` を離すと消える。
#[test]
fn dig_beam_follows_the_aim() {
    let mut arena = Arena::new(1, '#', "strong");
    let row = arena.row();
    arena.dig();
    let beam = arena.game.dig_beam(arena.player).expect("光線が出る");
    assert!(beam.hit);
    assert!((beam.to[1] - (row as f32 + 0.5)).abs() < 1.5, "同じ高さへ進む: {beam:?}");
    assert!(beam.to[0] > beam.from[0] + 15.0, "右へ進む");
    // 上へ向ければ、壁には当たらない。
    arena.step(GameInput { dig: true, aim: (0, -1000), ..Default::default() });
    let up = arena.game.dig_beam(arena.player).unwrap();
    assert!(!up.hit, "空へ向けると何も掘れない");
    arena.step(GameInput::default());
    assert!(arena.game.dig_beam(arena.player).is_none());
}

/// 道具を持たないキャラクターは掘れない。定義にない道具名は拒否する。
#[test]
fn dig_needs_an_equipped_tool_and_a_defined_name() {
    let mut arena = Arena::new(1, '#', "laser");
    arena.game.digs.clear();
    let before = testkit::count(&arena.world, '#');
    for _ in 0..200 {
        arena.dig();
    }
    assert_eq!(testkit::count(&arena.world, '#'), before, "道具がなければ掘れない");
    let err = arena.game.equip_dig(arena.player, &arena.defs, "nothing").unwrap_err();
    assert_eq!(err, GameError::UnknownDig("nothing".to_owned()));
}

/// 掘る道具の定義: 追加・上書き (`replace`)・範囲の検査・内容ハッシュ。
#[test]
fn dig_definitions_layer_and_replace() {
    let materials = MaterialTable::builtin().unwrap();
    let mut defs = Definitions::engine_defaults(&materials).unwrap();
    let laser = defs.dig("laser").unwrap().clone();
    assert_eq!((laser.power, laser.radius, laser.range, laser.recover), (6, 5, 48, 2));
    let base = defs.content_hash();
    defs.layer(
        DefinitionFiles { digs: Some(DIGS), ..Default::default() },
        &materials,
        Some("game"),
    )
    .unwrap();
    assert!(defs.dig("game:weak").is_some() && defs.dig("laser").is_some());
    assert_ne!(defs.content_hash(), base, "定義が変わるとハッシュが変わる");
    // 既定を上書きする。
    let replaced = r#"[(name: "laser", replace: true, power: 50, radius: 2, range: 10)]"#;
    defs.layer(DefinitionFiles { digs: Some(replaced), ..Default::default() }, &materials, None)
        .unwrap();
    assert_eq!(defs.dig("laser").unwrap().power, 50);
    // 範囲外・存在しない上書き・重複は拒否し、何も変えない。
    let hash = defs.content_hash();
    for bad in [
        r#"[(name: "bad", power: 0, radius: 3, range: 10)]"#,
        r#"[(name: "bad", power: 5, radius: 0, range: 10)]"#,
        r#"[(name: "bad", power: 5, radius: 3, range: 0)]"#,
        r#"[(name: "ghost", replace: true, power: 5, radius: 3, range: 10)]"#,
        r#"[(name: "d", power: 5, radius: 3, range: 10), (name: "d", power: 5, radius: 3, range: 10)]"#,
    ] {
        let result =
            defs.layer(DefinitionFiles { digs: Some(bad), ..Default::default() }, &materials, None);
        assert!(result.is_err(), "{bad}");
    }
    assert_eq!(defs.content_hash(), hash);
}

/// 同じ seed・入力なら、世界の中身とゲーム状態のハッシュが毎 tick 一致する。
#[test]
fn dig_is_deterministic() {
    let run = || {
        let mut arena = Arena::new(3, '#', "laser");
        let mut hashes = Vec::new();
        for tick in 0..150 {
            let input = if tick % 50 < 40 {
                GameInput { dig: true, aim: (1000, -(tick % 7) * 20), ..Default::default() }
            } else {
                GameInput::default()
            };
            arena.step(input);
            hashes.push((arena.world.state_hash(), arena.game.state_hash()));
        }
        hashes
    };
    assert_eq!(run(), run());
}
