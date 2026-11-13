//! 仕様 22: 武器・防具を持つ敵・戦利品・ボス・自動のスポーンと削除。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::{DefinitionFiles, Definitions};
use charon_game::game::{GameError, GameEvent, GameInput, GameState};
use charon_game::items::EquipSlot;
use charon_game::spawner::{SpawnRule, Spawner};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld, PlayerInput};
use charon_physics::PhysicsEngine;

const TEST_ENEMIES: &str = r#"[
    (name: "picker", width: 16, height: 24, max_hp: 50, walk_speed: 0, jump_speed: 0, sight: 1,
     attack: None, weapon: Some("wood_sword"),
     armor: ["wood_helmet", "stone_helmet", "wood_chestplate", "fire_ring"]),
    (name: "broken", width: 16, height: 24, max_hp: 50, walk_speed: 0, jump_speed: 0, sight: 1,
     attack: None, weapon: Some("no_such_weapon")),
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
    /// 床 (y = 40) が x = -320..320 に続く世界。プレイヤーは x = 10。
    fn new() -> Self {
        let materials = MaterialTable::builtin().unwrap();
        let mut defs = Definitions::engine_defaults(&materials).unwrap();
        let files = DefinitionFiles { enemies: Some(TEST_ENEMIES), ..Default::default() };
        defs.layer(files, &materials, Some("t")).unwrap();
        let mut world = World::new(Arc::new(materials), 1);
        for cx in -5..=5 {
            world.insert_chunk(ChunkPos::new(cx, 0));
        }
        testkit::fill(&mut world, -320, 40, 319, 40, '#');
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(10 * 16, 10 * 16, &world).unwrap();
        let mut game = GameState::from_definitions(&defs);
        game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
        game.give_inventory(player).unwrap();
        let mut arena =
            Self { world, physics: PhysicsEngine::new(), entities, game, defs, player, tick: 0 };
        for _ in 0..30 {
            arena.tick();
        }
        arena
    }

    fn tick(&mut self) {
        self.entities.step_players(&[(self.player, PlayerInput::default())], &self.world).unwrap();
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
        let defs = self.defs.clone();
        self.game.step_spawner(&self.world, &mut self.entities, &defs, self.tick, seed);
        self.world.step();
        self.tick += 1;
    }

    fn spawn(&mut self, name: &str, x: i32) -> Result<EntityId, GameError> {
        self.game.spawn_enemy(&self.defs, &mut self.entities, &self.world, name, (x, 10))
    }

    fn wear(&mut self, names: &[&str]) {
        for name in names {
            let item = self.defs.item_id(name, self.world.materials()).unwrap();
            self.game.inventories.get_mut(&self.player).unwrap().add_item(item, 1);
            let defs = self.defs.clone();
            self.game.equip(self.player, item, &defs).unwrap();
        }
    }

    fn hp(&self, id: EntityId) -> i32 {
        self.game.vitals.get(id).unwrap().hp
    }
}

/// 敵は、持っている武器・防具を、枠ごとにいちばん強いものを選んで装備する。防具の防御は体力の表に写る。
#[test]
fn enemies_auto_equip_the_best_carried_gear() {
    let mut a = Arena::new();
    let id = a.spawn("t:picker", 40).unwrap();
    let eq = &a.game.equipment[&id];
    let name =
        |slot| eq.get(slot).and_then(|item| a.defs.item_name(item, a.world.materials())).unwrap();
    assert_eq!(name(EquipSlot::Weapon), "wood_sword");
    assert_eq!(name(EquipSlot::Head), "stone_helmet", "強い兜を選ぶ");
    assert_eq!(name(EquipSlot::Body), "wood_chestplate");
    assert_eq!(name(EquipSlot::Accessory), "fire_ring");
    assert!(eq.get(EquipSlot::Legs).is_none());
    a.tick();
    let armor = a.game.vitals.get(id).unwrap().armor;
    assert_eq!((armor.defense, armor.fire), (4 + 3, 25));
}

#[test]
fn a_bad_gear_name_fails_without_creating_the_enemy() {
    let mut a = Arena::new();
    let before = (a.game.state_hash(), a.entities.state_hash());
    assert_eq!(a.spawn("t:broken", 40), Err(GameError::UnknownItem("no_such_weapon".to_owned())));
    assert_eq!((a.game.state_hash(), a.entities.state_hash()), before, "何も変わらない");
    assert!(a.game.enemies.is_empty());
}

/// 骸骨は、近づいて剣を振る。プレイヤーの防具が、受けるダメージを減らす。
#[test]
fn a_skeleton_swings_and_armor_reduces_the_damage() {
    let loss = |armored: bool| {
        let mut a = Arena::new();
        if armored {
            a.wear(&["stone_helmet", "stone_chestplate", "stone_greaves"]);
        }
        a.spawn("skeleton", 40).unwrap();
        for _ in 0..240 {
            a.tick();
        }
        MAX_HP - a.hp(a.player)
    };
    let (bare, armored) = (loss(false), loss(true));
    assert!(bare > 0, "骸骨に斬られる: {bare}");
    assert!(armored < bare, "防具で減る: {armored} < {bare}");
}

/// 弓兵は、距離をとって矢を撃つ。
#[test]
fn an_archer_keeps_its_distance_and_shoots_arrows() {
    let mut a = Arena::new();
    let archer = a.spawn("archer", 60).unwrap();
    let mut max_arrows = 0;
    for _ in 0..200 {
        a.tick();
        max_arrows = max_arrows.max(a.game.projectiles.len());
    }
    assert!(max_arrows > 0, "矢が出る");
    assert!(a.hp(a.player) < MAX_HP, "矢が当たる: {}", a.hp(a.player));
    let (px, ax) = (a.entities.get(a.player).unwrap().x16, a.entities.get(archer).unwrap().x16);
    assert!((ax - px).abs() > 30 * 16, "近づきすぎない: {}", (ax - px) / 16);
}

/// 倒した敵は戦利品を落とす。ボスは `BossDefeated` を出す。
#[test]
fn defeated_enemies_drop_loot_and_bosses_announce_themselves() {
    let mut a = Arena::new();
    let skeleton = a.spawn("skeleton", 100).unwrap();
    let golem = a.spawn("golem", 140).unwrap();
    a.game.take_events();
    a.game.vitals.get_mut(skeleton).unwrap().hp = 0;
    a.game.vitals.get_mut(golem).unwrap().hp = 0;
    a.tick();
    let events = a.game.take_events();
    assert!(events.iter().any(|e| matches!(e, GameEvent::Defeated(id, _) if *id == skeleton)));
    assert!(events.iter().any(|e| matches!(e, GameEvent::BossDefeated(id, _) if *id == golem)));
    assert!(
        !events.iter().any(|e| matches!(e, GameEvent::BossDefeated(id, _) if *id == skeleton)),
        "骸骨はボスではない"
    );
    let stone = a.world.materials().id("stone").unwrap();
    let gold = a.world.materials().id("gold_ore").unwrap();
    let dropped = |m| a.game.drops.iter().filter(|d| d.material == m).map(|d| d.count).sum::<u32>();
    assert_eq!(dropped(stone), 15);
    assert_eq!(dropped(gold), 40);
    assert!(a.game.enemies.is_empty() && !a.game.equipment.contains_key(&golem));
}

/// 魔術師は、火の玉を撃つ。
#[test]
fn a_mage_throws_fireballs() {
    let mut a = Arena::new();
    a.spawn("mage", 60).unwrap();
    let mut saw = false;
    for _ in 0..400 {
        a.tick();
        saw |= !a.game.projectiles.is_empty();
    }
    assert!(saw, "火の玉が出る");
    assert!(testkit::count(&a.world, 'f') + testkit::count(&a.world, 'k') > 0, "火か煙がある");
}

/// 自動のスポーン: プレイヤーのまわりの画面の外に出て、上限を守り、地面に立つ。遠くの敵は消える。決定的。
#[test]
fn the_spawner_spawns_around_the_player_and_despawns_far_enemies() {
    let run = || {
        let mut a = Arena::new();
        a.game.spawner = Some(Spawner {
            rules: vec![SpawnRule::new("walker", 0, 100, 1)],
            max_enemies: 3,
            interval: 10,
            near: 100,
            far: 160,
            despawn: 200,
        });
        let mut seen = 0;
        let mut max_enemies = 0;
        for _ in 0..800 {
            a.tick();
            max_enemies = max_enemies.max(a.game.enemies.len());
            for id in a.game.enemies.keys() {
                let e = a.entities.get(*id).unwrap();
                let px = a.entities.get(a.player).unwrap().x16;
                let dx = (e.x16 - px).abs() / 16;
                assert!(dx <= 200 + 16, "遠すぎる敵は消える: {dx}");
                seen += 1;
            }
        }
        assert!(seen > 0, "敵が出る");
        assert!(max_enemies <= 3, "上限を守る: {max_enemies}");
        (a.game.state_hash(), a.entities.state_hash())
    };
    assert_eq!(run(), run());

    // 遠すぎる敵は、倒さずに消える (戦利品なし・出来事なし)。
    let mut a = Arena::new();
    let far = a.spawn("skeleton", 250).unwrap();
    a.game.spawner = Some(Spawner { despawn: 100, interval: 1_000_000, ..Spawner::default() });
    a.game.take_events();
    a.tick();
    assert!(!a.game.enemies.contains_key(&far));
    assert!(a.entities.get(far).is_none());
    assert!(a.game.drops.is_empty());
    assert!(!a.game.take_events().iter().any(|e| matches!(e, GameEvent::Defeated(..))));
}

/// 深さ (y) の規則: 範囲の外の敵は選ばれない。ボスは同時に 1 体まで。
#[test]
fn spawn_rules_respect_depth_and_bosses_are_unique() {
    let mut a = Arena::new();
    let mut boss = SpawnRule::new("golem", 0, 100, 1);
    boss.boss = true;
    a.game.spawner = Some(Spawner {
        rules: vec![SpawnRule::new("walker", 500, 600, 1), boss],
        max_enemies: 5,
        interval: 5,
        near: 100,
        far: 160,
        despawn: 400,
    });
    let mut bosses = 0;
    for _ in 0..3000 {
        a.tick();
        bosses = bosses.max(a.game.enemies.values().filter(|e| e.def.boss).count());
        assert!(a.game.enemies.values().all(|e| e.def.boss), "深さの範囲外の敵 (walker) は出ない");
    }
    assert_eq!(bosses, 1, "ボスは出て、同時に 1 体まで");
}
