use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::world::World;
use charon_game::game::GameState;
use charon_game::{EntityId, EntityWorld};

use super::*;
use crate::api::Effects;

struct Rig {
    world: World,
    game: GameState,
    entities: EntityWorld,
    player: EntityId,
    effects: Effects,
}

impl Rig {
    fn new() -> Self {
        let materials = MaterialTable::builtin().unwrap();
        let mut world = World::new(Arc::new(materials), 1);
        world.insert_chunk(ChunkPos::new(0, 0));
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(10 * 16, 20 * 16, &world).unwrap();
        Self {
            world,
            game: GameState::new(Default::default()),
            entities,
            player,
            effects: Effects::default(),
        }
    }

    fn start(&mut self, script: &mut ScriptGame) {
        let mut ctx = Context::for_test(
            &mut self.world,
            &mut self.game,
            &mut self.entities,
            Some(self.player),
            &mut self.effects,
        );
        script.on_start(&mut ctx);
    }

    fn tick(&mut self, script: &mut ScriptGame, events: &[GameEvent]) {
        let mut ctx = Context::for_test(
            &mut self.world,
            &mut self.game,
            &mut self.entities,
            Some(self.player),
            &mut self.effects,
        );
        script.on_tick(&mut ctx, events);
    }
}

#[test]
fn scripts_read_the_snapshot_and_queue_commands() {
    let mut script = ScriptGame::new(
        r#"
        fn on_start() { give("stone", 7); play("jump"); }
        fn on_tick() {
            if tick() % 2 == 0 { shake(0.25); }
            if player_x() == 10 && has_player() { set_cell(30, 30, "stone"); }
        }
        "#,
    )
    .unwrap();
    let mut rig = Rig::new();
    rig.start(&mut script);
    rig.tick(&mut script, &[]);
    let stone = rig.world.materials().id("stone").unwrap();
    assert_eq!(rig.game.inventories[&rig.player].count(stone), 7);
    assert_eq!(rig.effects.sounds, vec![SoundEvent::Jump]);
    assert!((rig.effects.shake - 0.25).abs() < 1e-6, "tick 0 は偶数");
    assert_eq!(rig.world.get(CellPos::new(30, 30)).material, stone, "セルを置ける");
}

#[test]
fn events_reach_on_event_with_kind_and_position() {
    let mut script = ScriptGame::new(
        r#"
        fn on_event(kind, a, b) {
            if kind == "dug" { give("stone", a + b); }
            if kind == "defeated" { shake(1.0); }
        }
        "#,
    )
    .unwrap();
    let mut rig = Rig::new();
    let stone = rig.world.materials().id("stone").unwrap();
    let dug = GameEvent::Dug(charon_game::dig::Dug { pos: CellPos::new(3, 4), material: stone });
    rig.tick(&mut script, &[dug, GameEvent::Defeated(rig.player, CellPos::new(5, 6))]);
    assert_eq!(rig.game.inventories[&rig.player].count(stone), 7);
    assert_eq!(rig.effects.shake, 1.0);
}

#[test]
fn bad_scripts_are_contained() {
    // 文法のエラーは読み込みで知らせる。
    assert!(ScriptGame::new("fn on_tick( {").is_err());
    let mut rig = Rig::new();
    // 無限ループは命令の上限で止まり、ゲームは続く。
    let mut spin = ScriptGame::new("fn on_tick() { loop { } }").unwrap();
    rig.tick(&mut spin, &[]);
    assert_eq!(spin.errors, 1);
    // エラーが続くと、そのスクリプトは止まる。
    for _ in 0..MAX_ERRORS + 5 {
        rig.tick(&mut spin, &[]);
    }
    assert!(spin.halted());
    assert_eq!(spin.errors, MAX_ERRORS);
    // 知らない材質・音は無視される。命令の数にも上限がある。
    let mut noisy = ScriptGame::new(
        r#"fn on_tick() {
            give("nope", 5); play("nope"); set_cell(1, 1, "nope");
            for i in 0..1000 { shake(0.0); }
        }"#,
    )
    .unwrap();
    rig.tick(&mut noisy, &[]);
    assert_eq!(noisy.errors, 0);
    assert!(rig.game.inventories.get(&rig.player).is_none_or(|i| i.total() == 0));
}

/// 同梱のデモ MOD のスクリプトは、読めて、敵を倒すと音と揺れを頼む。
#[test]
fn the_demo_mod_script_works() {
    let source = include_str!("../../../../assets/mods/fse_demo/script.rhai");
    let mut script = ScriptGame::new(source).unwrap();
    let mut rig = Rig::new();
    rig.tick(&mut script, &[GameEvent::Defeated(rig.player, CellPos::new(5, 6))]);
    assert_eq!(script.errors, 0);
    assert!(rig.effects.sounds.is_empty());
    assert!(rig.effects.shake > 0.0);
    assert_eq!(rig.game.drops.len(), 1, "倒した場所に石が落ちる");
    let stone = rig.world.materials().id("stone").unwrap();
    let pickup = charon_game::drops::Pickup { who: rig.player, material: stone, count: 3 };
    rig.tick(&mut script, &[GameEvent::Pickup(pickup)]);
    assert_eq!(rig.effects.sounds, vec![SoundEvent::Pickup], "拾ったら音");
}

#[test]
fn scripts_cannot_reach_time_or_files() {
    // 時間やファイルの関数は登録していないので、呼ぶとエラーになる。
    let mut script = ScriptGame::new(r#"fn on_tick() { let t = timestamp(); }"#).unwrap();
    let mut rig = Rig::new();
    rig.tick(&mut script, &[]);
    assert_eq!(script.errors, 1, "知らない関数はエラー");
    let mut printer = ScriptGame::new(r#"fn on_tick() { print("hi"); }"#).unwrap();
    rig.tick(&mut printer, &[]);
    assert_eq!(printer.take_log(), vec!["hi".to_string()]);
}
