//! 仕様 10 §13: 持ち物とゲーム状態の保存。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use charon_core::cell::MaterialId;
use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::Definitions;
use charon_game::game::GameState;
use charon_game::save::{GAME_SAVE_FILE, GAME_SAVE_VERSION, GameSave, PlayerSave, SaveError};
use charon_game::vitals::{BREATH_MAX, MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld};

struct Setup {
    world: World,
    entities: EntityWorld,
    game: GameState,
    player: EntityId,
}

fn setup(x: i64, y: i64) -> Setup {
    let materials = MaterialTable::builtin().unwrap();
    let defs = Definitions::engine_defaults(&materials).unwrap();
    let mut world = World::new(Arc::new(materials), 1);
    world.insert_chunk(ChunkPos::new(0, 0));
    testkit::fill(&mut world, 0, 60, 63, 60, '#');
    let mut entities = EntityWorld::new();
    let player = entities.spawn_player(x * 16, y * 16, &world).unwrap();
    let mut game = GameState::from_definitions(&defs);
    game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
    game.give_inventory(player).unwrap();
    Setup { world, entities, game, player }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "charon-save-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 状態を写して RON にし、読み戻すと、新しいキャラクターへ体力・息・マナ・位置・持ち物が戻る。
#[test]
fn game_state_roundtrips_through_ron() {
    let mut a = setup(10, 40);
    let stone = testkit::material_of(&a.world, '#');
    let sand = testkit::material_of(&a.world, 's');
    a.game.inventories.get_mut(&a.player).unwrap().add(stone, 42);
    a.game.inventories.get_mut(&a.player).unwrap().add(sand, 7);
    a.game.vitals.get_mut(a.player).unwrap().hp = 37;
    a.game.vitals.get_mut(a.player).unwrap().breath = 123;
    a.game.wands.get_mut(&a.player).unwrap().mana = 55;
    let materials = Arc::clone(a.world.materials());
    let save = GameSave::capture(&a.game, &a.entities, a.player, &materials, 999);
    assert_eq!(save.version, GAME_SAVE_VERSION);
    assert_eq!(save.tick, 999);
    let text = save.to_ron().unwrap();
    let loaded = GameSave::from_ron(&text).unwrap();
    assert_eq!(loaded, save);

    // 別の場所に生まれた新しいキャラクターへ戻す。
    let mut b = setup(30, 40);
    let applied = loaded.apply(&mut b.game, &mut b.entities, &b.world, b.player);
    assert!(applied.moved);
    assert_eq!(applied.kinds, 2);
    let entity = b.entities.get(b.player).unwrap();
    assert_eq!((entity.x16, entity.y16), (10 * 16, 40 * 16), "位置が戻る");
    let vitals = b.game.vitals.get(b.player).unwrap();
    assert_eq!((vitals.hp, vitals.breath), (37, 123));
    assert_eq!(b.game.wands[&b.player].mana, 55);
    let inventory = &b.game.inventories[&b.player];
    assert_eq!((inventory.count(stone), inventory.count(sand)), (42, 7));
}

/// 倒れていたプレイヤーは満タンで戻る。保存した位置が固体と重なるときは、位置を戻さない。値は上限に収める。
#[test]
fn apply_is_careful_with_dead_players_solid_positions_and_limits() {
    let mut s = setup(10, 40);
    let save = GameSave {
        version: GAME_SAVE_VERSION,
        tick: 0,
        player: Some(PlayerSave {
            hp: 0,
            breath: u16::MAX,
            mana: u32::MAX,
            x16: 10 * 16,
            y16: 60 * 16, // 床の中
            inventory: vec![("stone".to_owned(), 5), ("not_a_material".to_owned(), 9)],
        }),
    };
    let applied = save.apply(&mut s.game, &mut s.entities, &s.world, s.player);
    assert!(!applied.moved, "固体の中へは戻さない");
    let entity = s.entities.get(s.player).unwrap();
    assert_eq!((entity.x16, entity.y16), (10 * 16, 40 * 16));
    let vitals = s.game.vitals.get(s.player).unwrap();
    assert_eq!(vitals.hp, MAX_HP, "体力 0 は満タンで戻る");
    assert_eq!(vitals.breath, BREATH_MAX, "息は上限に収める");
    let wand = &s.game.wands[&s.player];
    assert_eq!(wand.mana, wand.mana_max, "マナは上限に収める");
    assert_eq!(applied.kinds, 1, "知らない材質の名前は読み飛ばす");
    let stone = testkit::material_of(&s.world, '#');
    assert_eq!(s.game.inventories[&s.player].count(stone), 5);
    // 体力が多すぎる値は、上限に収める。
    let big = GameSave {
        player: Some(PlayerSave { hp: MAX_HP * 5, ..save.player.clone().unwrap() }),
        ..save
    };
    big.apply(&mut s.game, &mut s.entities, &s.world, s.player);
    assert_eq!(s.game.vitals.get(s.player).unwrap().hp, MAX_HP);
}

#[test]
fn bad_game_saves_are_rejected() {
    let good = GameSave { version: GAME_SAVE_VERSION, tick: 1, player: None }.to_ron().unwrap();
    assert!(GameSave::from_ron(&good).is_ok());
    let old = good.replace(&format!("version: {GAME_SAVE_VERSION}"), "version: 999");
    assert_eq!(GameSave::from_ron(&old), Err(SaveError::Version(999)));
    assert!(matches!(GameSave::from_ron("not ron"), Err(SaveError::Parse(_))));
    let many: Vec<(String, u32)> = (0..300).map(|i| (format!("m{i}"), 1)).collect();
    let flood = GameSave {
        version: GAME_SAVE_VERSION,
        tick: 0,
        player: Some(PlayerSave { hp: 1, breath: 1, mana: 0, x16: 0, y16: 0, inventory: many }),
    };
    let text = flood.to_ron().unwrap();
    assert!(matches!(GameSave::from_ron(&text), Err(SaveError::Invalid(_))));
}

/// `game.ron` の書き込みと読み込み: ないときは `None`、壊れていればエラー、書き直しても一時ファイルは残らない。
#[test]
fn game_save_file_io() {
    let dir = TempDir::new();
    assert_eq!(GameSave::read(&dir.0), Ok(None), "ファイルがなければ None");
    let s = setup(10, 40);
    let materials = Arc::clone(s.world.materials());
    let save = GameSave::capture(&s.game, &s.entities, s.player, &materials, 5);
    save.write(&dir.0).unwrap();
    save.write(&dir.0).unwrap();
    assert_eq!(GameSave::read(&dir.0).unwrap(), Some(save));
    let names: Vec<_> = std::fs::read_dir(&dir.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, [GAME_SAVE_FILE], "一時ファイルは残らない");
    std::fs::write(dir.0.join(GAME_SAVE_FILE), "garbage").unwrap();
    assert!(GameSave::read(&dir.0).is_err());
}

/// 持ち物のないキャラクターは掘っても集めない。持ち物があれば、掘ったセルの数だけ材質が増える。
#[test]
fn inventory_requires_a_character_with_a_wand() {
    let mut s = setup(10, 40);
    assert!(s.game.give_inventory(EntityId(99)).is_err(), "杖のないキャラクターには持たせない");
    s.game.inventories.get_mut(&s.player).unwrap().add(MaterialId(3), 1);
    s.game.give_inventory(s.player).unwrap();
    assert_eq!(s.game.inventories[&s.player].count(MaterialId(3)), 1, "すでにあるときは中身を残す");
}
