//! 仕様 20: 戦闘 (近接・遠距離・杖の武器、防具の軽減、属性、ノックバック、火の玉)。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::combat::{MELEE_COOLDOWN, RANGED_COOLDOWN};
use charon_game::defs::{DefinitionFiles, Definitions};
use charon_game::game::{GameEvent, GameInput, GameState};
use charon_game::items::WeaponClass;
use charon_game::vitals::{DamageKind, MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld, PlayerInput};
use charon_physics::PhysicsEngine;

const TEST_ENEMIES: &str = r#"[
    (name: "dummy", width: 8, height: 16, max_hp: 100, walk_speed: 0, jump_speed: 0,
     sight: 1, attack: None),
]"#;

const FIREPROOF: &str =
    r#"[(name: "fireproof", kind: Armor(slot: Accessory, defense: 0, fire: 100))]"#;

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
    /// 床 (y = 40) の上に、プレイヤー (x = 10) を置く。
    fn new() -> Self {
        let materials = MaterialTable::builtin().unwrap();
        let mut defs = Definitions::engine_defaults(&materials).unwrap();
        let files = DefinitionFiles {
            enemies: Some(TEST_ENEMIES),
            items: Some(FIREPROOF),
            ..Default::default()
        };
        defs.layer(files, &materials, Some("t")).unwrap();
        let mut world = World::new(Arc::new(materials), 1);
        world.insert_chunk(ChunkPos::new(0, 0));
        testkit::fill(&mut world, 0, 40, 63, 40, '#');
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(10 * 16, 10 * 16, &world).unwrap();
        let mut game = GameState::from_definitions(&defs);
        game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
        game.give_inventory(player).unwrap();
        let mut arena =
            Self { world, physics: PhysicsEngine::new(), entities, game, defs, player, tick: 0 };
        for _ in 0..30 {
            arena.tick(false, (1000, 0));
        }
        arena
    }

    fn give_and_equip(&mut self, name: &str) {
        let item = self.defs.item_id(name, self.world.materials()).unwrap();
        self.game.inventories.get_mut(&self.player).unwrap().add_item(item, 1);
        let defs = self.defs.clone();
        self.game.equip(self.player, item, &defs).unwrap();
    }

    fn dummy(&mut self, x: i32) -> EntityId {
        self.game
            .spawn_enemy(&self.defs, &mut self.entities, &self.world, "t:dummy", (x, 24))
            .unwrap()
    }

    fn tick(&mut self, fire: bool, aim: (i32, i32)) {
        self.entities.step_players(&[(self.player, PlayerInput::default())], &self.world).unwrap();
        let seed = self.world.seed();
        let input = GameInput { fire, aim, ..Default::default() };
        self.game
            .step(
                &mut self.world,
                &mut self.physics,
                &mut self.entities,
                &[(self.player, input)],
                self.tick,
                seed,
            )
            .unwrap();
        self.world.step();
        self.tick += 1;
    }

    fn hp(&self, id: EntityId) -> i32 {
        self.game.vitals.get(id).unwrap().hp
    }
}

/// 物理は防御で減る (ただし元の 1/4 まで)。属性は耐性 (%) で減る (四捨五入)。
#[test]
fn damage_kinds_are_reduced_by_defense_and_resist() {
    let mut v = Vitals::new(1000).unwrap();
    assert_eq!(v.damage_kind(10, DamageKind::Physical), 10, "防具なし");
    v.armor.defense = 6;
    v.armor.fire = 25;
    v.armor.poison = 100;
    assert_eq!(v.damage_kind(10, DamageKind::Physical), 4, "10 − 6");
    assert_eq!(v.damage_kind(4, DamageKind::Physical), 1, "元の 1/4 (1 以上) までしか減らない");
    assert_eq!(v.damage_kind(40, DamageKind::Physical), 34);
    assert_eq!(v.damage_kind(20, DamageKind::Fire), 15, "25% 減");
    assert_eq!(v.damage_kind(50, DamageKind::Poison), 0, "耐性 100% は無効");
    assert_eq!(v.damage_kind(8, DamageKind::Cold), 8, "冷気の耐性はなし");
    assert_eq!(v.damage_kind(0, DamageKind::Physical), 0);
    assert_eq!(v.damage_kind(-5, DamageKind::Fire), 0);
    let mut low = Vitals::new(10).unwrap();
    assert_eq!(low.damage_kind(100, DamageKind::Physical), 10, "体力を超えては減らない");
    assert!(!low.alive());
}

/// 防具を付けると、体力の表の防御・耐性が次の tick で写り、取れば戻る。
#[test]
fn armor_is_copied_from_equipment_each_step() {
    let mut a = Arena::new();
    assert_eq!(a.game.vitals.get(a.player).unwrap().armor.defense, 0);
    a.give_and_equip("stone_chestplate");
    a.give_and_equip("stone_helmet");
    a.give_and_equip("fire_ring");
    a.tick(false, (1000, 0));
    let armor = a.game.vitals.get(a.player).unwrap().armor;
    assert_eq!((armor.defense, armor.fire, armor.poison), (6 + 4, 25, 0));
    let id = a.player;
    a.game.unequip(id, charon_game::items::EquipSlot::Body).unwrap();
    a.tick(false, (1000, 0));
    assert_eq!(a.game.vitals.get(a.player).unwrap().armor.defense, 4);
}

/// 近接の武器: 前の相手を斬る。間隔の間は斬れず、間隔のあとはまた斬る。後ろの相手には当たらない。ノックバックする。
#[test]
fn melee_hits_in_front_with_cooldown_and_knockback() {
    let mut a = Arena::new();
    a.give_and_equip("wood_sword");
    assert_eq!(a.game.equipped_weapon(a.player), Some((WeaponClass::Melee, 6)));
    let front = a.dummy(36);
    let behind = a.dummy(2);
    a.game.take_events();
    a.tick(true, (1000, 0));
    assert_eq!(a.hp(front), 100 - 6, "前の相手を斬る");
    assert_eq!(a.hp(behind), 100, "後ろには当たらない");
    assert!(
        a.entities.get(front).unwrap().ext_vx > 0 || a.entities.get(front).unwrap().x16 > 36 * 16
    );
    let events = a.game.take_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, GameEvent::Melee { target, damage: 6, .. } if *target == front))
    );
    // 間隔の間は、押し続けても斬れない。
    // (弾き飛ばされた相手は、毎 tick 前にもどして確かめる。)
    let put_back = |a: &mut Arena| {
        let e = a.entities.get_mut(front).unwrap();
        e.x16 = 36 * 16;
        e.ext_vx = 0;
    };
    for _ in 0..MELEE_COOLDOWN - 2 {
        put_back(&mut a);
        a.tick(true, (1000, 0));
    }
    assert_eq!(a.hp(front), 100 - 6, "間隔の間");
    for _ in 0..4 {
        put_back(&mut a);
        a.tick(true, (1000, 0));
    }
    assert!(a.hp(front) <= 100 - 12, "間隔のあと、また斬る: {}", a.hp(front));
    // 左向きなら、後ろにいた相手を斬る。
    a.tick(true, (-1000, 0));
    for _ in 0..MELEE_COOLDOWN {
        a.tick(true, (-1000, 0));
    }
    assert!(a.hp(behind) < 100, "左向きに斬る");
}

/// 遠距離の武器: 重力のある矢を撃つ。矢が当たると、物理ダメージと、弾の向きへのノックバック。
#[test]
fn ranged_weapon_fires_an_arrow_with_gravity() {
    let mut a = Arena::new();
    a.give_and_equip("wood_bow");
    assert_eq!(a.game.equipped_weapon(a.player), Some((WeaponClass::Ranged, 8)));
    let target = a.dummy(56);
    a.tick(true, (1000, -100));
    assert_eq!(a.game.projectiles.len(), 1, "矢が出る");
    let y0 = a.game.projectiles.iter().next().unwrap().y16;
    a.tick(false, (1000, 0));
    // 重力で、上向きの勢いが弱まっていく (y の変化が下向きに寄る)。
    let y1 = a.game.projectiles.iter().next().map_or(y0, |p| p.y16);
    assert!(y1 != y0);
    for _ in 0..RANGED_COOLDOWN {
        a.tick(false, (1000, 0));
    }
    for _ in 0..60 {
        a.tick(false, (1000, 0));
    }
    assert!(a.hp(target) < 100 || a.game.projectiles.is_empty(), "矢が当たるか、落ちる");
}

/// 弾が当たると、弾の向きへ弾き飛ばされる。
#[test]
fn projectile_hits_knock_the_target_back() {
    let mut a = Arena::new();
    let target = a.dummy(30);
    a.tick(true, (1000, 0));
    let mut pushed = false;
    for _ in 0..40 {
        a.tick(false, (1000, 0));
        let e = a.entities.get(target).unwrap();
        if e.ext_vx > 0 || e.x16 > 30 * 16 {
            pushed = true;
            break;
        }
    }
    assert!(pushed, "杖の弾でも、右へ弾かれる");
    assert!(a.hp(target) < 100);
}

/// 杖の武器を装備すると、そのキャラクターの杖が入れ替わる。火の玉は、木に火をつけて燃え広がらせる。
#[test]
fn a_fire_wand_swaps_the_wand_and_its_fireball_burns_wood() {
    let mut a = Arena::new();
    assert_eq!(a.game.wands[&a.player].spells.len(), 1);
    a.give_and_equip("fire_wand");
    let fireball = a.game.spells.id("fireball").unwrap();
    assert_eq!(a.game.wands[&a.player].spells, vec![fireball]);
    // 木の壁。
    testkit::fill(&mut a.world, 40, 20, 41, 39, 'W');
    let wood_before = testkit::count(&a.world, 'W');
    for tick in 0..400 {
        a.tick(tick < 40, (1000, 0));
    }
    let wood_after = testkit::count(&a.world, 'W');
    assert!(wood_after < wood_before, "火が燃え移って、木が減る: {wood_before} → {wood_after}");
    assert!(testkit::count(&a.world, 'h') > 0, "燃えた木は灰になる");
}

/// 火に包まれると体力が減り、燃えない耐性 (100%) の装備があれば減らない。
#[test]
fn fire_resist_protects_from_lava() {
    let run = |protected: bool| {
        let mut a = Arena::new();
        if protected {
            a.give_and_equip("t:fireproof");
        }
        testkit::fill(&mut a.world, 0, 30, 63, 39, 'l');
        for _ in 0..30 {
            a.tick(false, (1000, 0));
        }
        a.hp(a.player)
    };
    let (bare, safe) = (run(false), run(true));
    assert!(bare < MAX_HP, "溶岩で体力が減る: {bare}");
    assert_eq!(safe, MAX_HP, "耐性 100% なら減らない");
}

/// 同じ入力なら、同じ結果 (近接・遠距離・ノックバックを含めて)。
#[test]
fn combat_is_deterministic() {
    let run = || {
        let mut a = Arena::new();
        a.give_and_equip("stone_sword");
        a.dummy(36);
        for tick in 0..120 {
            a.tick(tick % 3 != 0, (1000, 0));
        }
        (a.game.state_hash(), a.entities.state_hash(), a.world.state_hash())
    };
    assert_eq!(run(), run());
}
