//! 外から見たときの公開 API (`prelude`) だけで、ゲームを書いて動かせること (仕様 14 §5)。
//! 名前が消えたり、使い方が変わったりすると、ここがコンパイルできなくなる。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_app::prelude::*;

/// 敵を倒した数を数えて、倒すたびに揺らし、拾う音を頼むだけの小さなゲーム。
#[derive(Default)]
struct Tiny {
    defeated: u32,
    picked: u32,
    unknown: u32,
}

impl Game for Tiny {
    fn on_tick(&mut self, ctx: &mut Context<'_>, events: &[GameEvent]) {
        for event in events {
            match event {
                GameEvent::Defeated(_, _) => {
                    self.defeated += 1;
                    ctx.shake(0.3);
                }
                GameEvent::Pickup(p) => {
                    self.picked += p.count;
                    ctx.play(SoundEvent::Pickup);
                }
                // 新しい種類の出来事が増えても、このゲームはコンパイルできる (`non_exhaustive`)。
                _ => self.unknown += 1,
            }
        }
    }
}

#[test]
fn a_game_can_be_written_with_the_prelude_alone() {
    let materials = MaterialTable::builtin().unwrap();
    let stone = materials.id("stone").unwrap();
    let mut world = World::new(Arc::new(materials), 1);
    let mut game = GameState::new(Default::default());
    let mut entities = EntityWorld::new();
    let mut effects = Effects::default();
    let mut tiny = Tiny::default();

    // 問い合わせ API も、prelude から使える。
    assert!(world.raycast(CellPos::new(0, 0), CellPos::new(3, 0), Stop::Solid).is_some());

    let id = {
        world.insert_chunk(charon_core::coords::ChunkPos::new(0, 0));
        entities.spawn_player(16, 16, &world).unwrap()
    };
    let events = [
        GameEvent::Defeated(id, CellPos::new(1, 2)),
        GameEvent::Pickup(Pickup { who: id, material: stone, count: 4 }),
    ];
    let mut ctx = Context::for_test(&mut world, &mut game, &mut entities, Some(id), &mut effects);
    tiny.on_tick(&mut ctx, &events);
    assert_eq!((tiny.defeated, tiny.picked, tiny.unknown), (1, 4, 0));
    assert_eq!(effects.sounds, vec![SoundEvent::Pickup]);
    assert!((effects.shake - 0.3).abs() < 1e-6);

    // ゲームを `AppOptions` に載せられる。
    let options = AppOptions { logic: Some(GameLogic::new(Tiny::default())), ..Default::default() };
    assert!(options.logic.is_some());
}

#[test]
fn the_prelude_covers_the_other_building_blocks() {
    // 設定・キー割り当て・カメラ・アセット・スクリプトが、すべて prelude から作れる。
    let _ = (Settings::default(), Action::Jump);
    let mut camera = CameraFx::default();
    camera.shake(0.5);
    assert!(camera.trauma() > 0.0);
    assert!(Assets::new().read("nope.png").is_err());
    assert!(matches!(Assets::new().sound("nope.wav"), Err(AssetError::File(_))));
    let spec = SpriteSpec::default();
    assert_eq!(spec.scale, 1.0);
    assert!(ScriptGame::new("fn on_tick() {}").is_ok());
    let def = LevitationDef::default();
    assert_eq!(Levitation::new(def).fuel, def.fuel_max);
    let _ = (Collapse::new(), Drops::new(), StatusTable::new(), StatusKind::Wet);
    let _ = PlayerLoadout {
        max_hp: 10,
        wand: String::new(),
        hook: None,
        dig: None,
        levitation: None,
        items: Vec::new(),
        hotbar: Vec::new(),
    };
    let _ = EnemySpawn { name: String::new(), cell: [0, 0] };
    let _ = (GameInput::default(), Clips::STILL, Clip::still(0), SoundBank::procedural(44_100));
    let _: Option<(Cell, MaterialId, KeyBindings, RayHit, Definitions)> = None;
}
