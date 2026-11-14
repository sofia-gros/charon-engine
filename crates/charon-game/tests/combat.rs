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
use charon_game::items::{EquipSlot, Equipment, ItemKind, WeaponClass};
use charon_game::save::GameSave;
use charon_game::vitals::{COMBAT_IMMUNITY_TICKS, DamageKind, MAX_HP, Vitals, step_environment};
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
        Self::with_seed(1)
    }

    fn with_seed(seed: u64) -> Self {
        Self::with_seed_and_items(seed, None)
    }

    fn with_seed_and_items(seed: u64, items: Option<&str>) -> Self {
        let materials = MaterialTable::builtin().unwrap();
        let mut defs = Definitions::engine_defaults(&materials).unwrap();
        let files = DefinitionFiles {
            enemies: Some(TEST_ENEMIES),
            items: Some(FIREPROOF),
            ..Default::default()
        };
        defs.layer(files, &materials, Some("t")).unwrap();
        if let Some(items) = items {
            defs.layer(
                DefinitionFiles { items: Some(items), ..Default::default() },
                &materials,
                Some("m"),
            )
            .unwrap();
        }
        let mut world = World::new(Arc::new(materials), seed);
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

/// 同 tick の近接と弾の直撃は一発分だけ通り、次に被弾できるのは 12 tick 目。
#[test]
fn combat_immunity_blocks_repeat_hits_and_expires() {
    for seed in 1..=8 {
        let mut a = Arena::with_seed(seed);
        a.give_and_equip("wood_sword");
        let target = a.dummy(36);
        let first = a
            .game
            .projectiles
            .spawn(
                &a.world,
                39 * 16,
                30 * 16,
                -160,
                0,
                0,
                8,
                17,
                charon_game::projectile::Payload::None,
            )
            .unwrap();
        a.tick(true, (1000, 0));
        assert_eq!(a.hp(target), 94, "seed {seed}: 同 tick の弾は斬撃後に無効");
        assert!(a.game.take_events().iter().any(
            |e| matches!(e, GameEvent::Hit(hit) if hit.projectile == first && hit.damage == 17)
        ));
        let reset = |a: &mut Arena| {
            let e = a.entities.get_mut(target).unwrap();
            e.x16 = 36 * 16;
            e.y16 = 24 * 16;
            e.vx = 0;
            e.vy = 0;
            e.ext_vx = 0;
        };
        reset(&mut a);
        let shot = |a: &mut Arena| {
            a.game
                .projectiles
                .spawn(
                    &a.world,
                    39 * 16,
                    30 * 16,
                    -160,
                    0,
                    0,
                    8,
                    17,
                    charon_game::projectile::Payload::None,
                )
                .unwrap()
        };
        let blocked = shot(&mut a);
        a.tick(false, (1000, 0));
        assert!(
            a.game
                .take_events()
                .iter()
                .any(|e| matches!(e, GameEvent::Hit(hit) if hit.projectile == blocked))
        );
        assert_eq!(a.hp(target), 94, "seed {seed}: 次 tick の弾は体力を減らさない");
        assert_eq!(a.entities.get(target).unwrap().ext_vx, 0, "免疫中はノックバックしない");
        for _ in 0..10 {
            a.tick(false, (1000, 0));
        }
        assert_eq!(a.game.vitals.get(target).unwrap().combat_immunity(), 1);
        reset(&mut a);
        shot(&mut a);
        a.tick(false, (1000, 0));
        assert_eq!(a.hp(target), 77, "seed {seed}: 12 tick 目の直撃は通る");
        let v = a.game.vitals.get_mut(target).unwrap();
        assert_eq!(v.combat_immunity(), COMBAT_IMMUNITY_TICKS);
        for left in (1..COMBAT_IMMUNITY_TICKS).rev() {
            v.tick_combat_immunity();
            assert_eq!(v.damage_combat(7, DamageKind::Physical), 0, "残り {left}");
            assert_eq!(v.combat_immunity(), left);
        }
        v.tick_combat_immunity();
        assert_eq!(v.damage_combat(7, DamageKind::Physical), 7);
        assert_eq!(v.combat_immunity(), COMBAT_IMMUNITY_TICKS);
    }
}

/// 熱・溺れ・毒は戦闘の免疫とは独立して進み、免疫の時計を延ばさない。
#[test]
fn combat_immunity_keeps_environment_damage() {
    for seed in 1..=8 {
        let mut a = Arena::with_seed(seed);
        let pos = charon_core::coords::CellPos::new(15, 26);
        let lava = a.world.materials().id("lava").unwrap();
        let hot = a.world.make_cell(lava, pos);
        a.world.set(pos, hot);
        let entity = a.entities.get(a.player).unwrap().clone();
        let vitals = a.game.vitals.get_mut(a.player).unwrap();
        assert_eq!(vitals.damage_combat(3, DamageKind::Physical), 3);
        let before = vitals.hp;
        step_environment(&entity, vitals, &a.world);
        assert!(vitals.hp < before, "seed {seed}: 免疫中も熱が通る");
        assert_eq!(vitals.combat_immunity(), COMBAT_IMMUNITY_TICKS);
        assert_eq!(vitals.damage_combat(10, DamageKind::Physical), 0);
        let empty = a.world.make_cell(charon_core::cell::MaterialId::EMPTY, pos);
        a.world.set(pos, empty);
        let head = i32::try_from(entity.y16.div_euclid(16)).unwrap();
        let left = i32::try_from(entity.x16.div_euclid(16)).unwrap();
        testkit::fill(&mut a.world, left, head, left + i32::from(entity.width) - 1, head, 'w');
        vitals.breath = 1;
        let before_drown = vitals.hp;
        for _ in 0..charon_game::vitals::DROWN_INTERVAL {
            step_environment(&entity, vitals, &a.world);
        }
        assert_eq!(vitals.hp, before_drown - 1, "seed {seed}: 溺れは防がない");
        let before_poison = vitals.hp;
        a.game.statuses.apply(a.player, charon_game::status::StatusKind::Poison, 30);
        a.game.statuses.step(a.player, &entity, vitals, &a.world);
        assert_eq!(vitals.hp, before_poison - 1, "seed {seed}: 毒も防がない");
        assert_eq!(vitals.combat_immunity(), COMBAT_IMMUNITY_TICKS);
    }
}

/// 免疫は同期ハッシュに含み、無効な入力では時計を進めず、復活後は消える。
#[test]
fn combat_immunity_is_hashed_and_cleared_on_respawn() {
    for seed in 1..=8 {
        let mut a = Arena::with_seed(seed);
        let baseline = a.game.state_hash();
        assert_eq!(
            a.game.vitals.get_mut(a.player).unwrap().damage_combat(2, DamageKind::Physical),
            2
        );
        let active = a.game.state_hash();
        assert_ne!(active, baseline);
        let invalid = (charon_game::EntityId(u64::MAX), GameInput::default());
        let before = (a.game.state_hash(), a.world.state_hash(), a.entities.state_hash());
        assert!(
            a.game
                .step(&mut a.world, &mut a.physics, &mut a.entities, &[invalid], a.tick, seed)
                .is_err()
        );
        assert_eq!(before, (a.game.state_hash(), a.world.state_hash(), a.entities.state_hash()));
        a.game.vitals.get_mut(a.player).unwrap().tick_combat_immunity();
        assert_ne!(active, a.game.state_hash(), "残り tick がハッシュに入る");
        a.game.vitals.get_mut(a.player).unwrap().hp = 0;
        for _ in 0..charon_game::respawn::RESPAWN_TICKS {
            a.game.step_respawn(&a.world, &mut a.entities, [10, 16]);
        }
        assert_eq!(a.game.vitals.get(a.player).unwrap().combat_immunity(), 0);
        let trace = |seed| {
            let mut b = Arena::with_seed(seed);
            b.game.vitals.get_mut(b.player).unwrap().damage_combat(2, DamageKind::Physical);
            for _ in 0..20 {
                b.tick(false, (1000, 0));
            }
            (b.game.state_hash(), b.entities.state_hash(), b.world.state_hash())
        };
        assert_eq!(trace(seed), trace(seed), "seed {seed}: 決定性");
    }
}

/// 頭・胴・脚の同じセットのみ頭の補正を一度加え、欠損や混成では付かない。
#[test]
fn armor_set_requires_all_three_pieces() {
    for seed in 1..=8 {
        let mut a = Arena::with_seed(seed);
        a.give_and_equip("stone_helmet");
        a.give_and_equip("stone_chestplate");
        a.tick(false, (1000, 0));
        assert_eq!(a.game.vitals.get(a.player).unwrap().armor.defense, 10, "seed {seed}: 2 部位");
        a.give_and_equip("wood_greaves");
        a.tick(false, (1000, 0));
        assert_eq!(a.game.vitals.get(a.player).unwrap().armor.defense, 12, "seed {seed}: 混成");
        a.give_and_equip("stone_greaves");
        a.tick(false, (1000, 0));
        let armor = a.game.vitals.get(a.player).unwrap().armor;
        assert_eq!((armor.defense, armor.cold), (18, 10), "seed {seed}: 石 3 部位");
        a.game.unequip(a.player, EquipSlot::Body).unwrap();
        a.tick(false, (1000, 0));
        let armor = a.game.vitals.get(a.player).unwrap().armor;
        assert_eq!((armor.defense, armor.cold), (8, 0), "seed {seed}: 外した次 tick");
        // MOD のセット ID は名前空間つきで照合し、上限の適用は最後に行う。
        let mut defs = a.defs.clone();
        let mod_set = r#"[
            (name: "h", kind: Armor(slot: Head, defense: 1000, set: Some("heavy"), set_defense: 1000, set_fire: 60)),
            (name: "b", kind: Armor(slot: Body, defense: 1000, set: Some("heavy"))),
            (name: "l", kind: Armor(slot: Legs, defense: 1000, set: Some("heavy"))),
            (name: "a", kind: Armor(slot: Accessory, defense: 1000, fire: 60)),
        ]"#;
        defs.layer(
            DefinitionFiles { items: Some(mod_set), ..Default::default() },
            a.world.materials(),
            Some("test"),
        )
        .unwrap();
        let mut equipment = Equipment::new();
        for (slot, name) in [
            (EquipSlot::Head, "test:h"),
            (EquipSlot::Body, "test:b"),
            (EquipSlot::Legs, "test:l"),
            (EquipSlot::Accessory, "test:a"),
        ] {
            equipment.set(slot, defs.item_id(name, a.world.materials()));
        }
        let stats = equipment.stats(|id| defs.item_def(id));
        assert_eq!((stats.defense, stats.resist.fire), (5000, 100));
        let before = defs.content_hash();
        for invalid in [
            r#"[(name: "bad", kind: Armor(slot: Accessory, set: Some("heavy")))]"#,
            r#"[(name: "bad", kind: Armor(slot: Body, set: Some("heavy"), set_defense: 1))]"#,
            r#"[(name: "bad", kind: Armor(slot: Head, set_defense: 1))]"#,
            r#"[(name: "bad", kind: Armor(slot: Head, set: Some("heavy"), set_fire: 101))]"#,
            r#"[(name: "bad", kind: Armor(slot: Head, set: Some("")))]"#,
            r#"[(name: "stone_helmet", replace: true, kind: Armor(slot: Head, set: Some("a::b")))]"#,
        ] {
            assert!(
                defs.layer(
                    DefinitionFiles { items: Some(invalid), ..Default::default() },
                    a.world.materials(),
                    Some("test")
                )
                .is_err()
            );
            assert_eq!(defs.content_hash(), before, "seed {seed}: 不正な層は原子的");
        }
        assert!(matches!(
            defs.item_def(defs.item_id("test:h", a.world.materials()).unwrap()).unwrap().kind,
            ItemKind::Armor { set: Some(_), .. }
        ));
    }
}

/// アクセサリーの補正は毎 tick 上書きされ、交換しても残らない。セット効果と合算する。
#[test]
fn accessory_bonus_does_not_stack_on_refresh() {
    for seed in 1..=8 {
        let mut a = Arena::with_seed(seed);
        for name in ["wood_helmet", "wood_chestplate", "wood_greaves", "fire_ring"] {
            a.give_and_equip(name);
        }
        for _ in 0..30 {
            a.tick(false, (1000, 0));
            let armor = a.game.vitals.get(a.player).unwrap().armor;
            assert_eq!((armor.defense, armor.fire), (9, 25), "seed {seed}: 加算し続けない");
        }
        a.give_and_equip("poison_charm");
        a.tick(false, (1000, 0));
        let armor = a.game.vitals.get(a.player).unwrap().armor;
        assert_eq!((armor.defense, armor.fire, armor.poison), (9, 0, 25));
        a.game.unequip(a.player, EquipSlot::Accessory).unwrap();
        a.tick(false, (1000, 0));
        let armor = a.game.vitals.get(a.player).unwrap().armor;
        assert_eq!((armor.defense, armor.poison), (9, 0));
    }
}

/// 装備名だけを game.ron に保存し、復帰後の tick に新しい定義から補正を復元する。
#[test]
fn equipment_bonuses_survive_item_save() {
    for seed in 1..=8 {
        let mut a = Arena::with_seed(seed);
        for name in ["stone_helmet", "stone_chestplate", "stone_greaves", "fire_ring"] {
            a.give_and_equip(name);
        }
        a.tick(false, (1000, 0));
        let expected = a.game.vitals.get(a.player).unwrap().armor;
        let saved = GameSave::capture(&a.game, &a.entities, a.player, a.world.materials(), a.tick)
            .with_items(&a.game, a.player, &a.defs, a.world.materials());
        assert_eq!(saved.version, charon_game::save::GAME_SAVE_VERSION);
        let saved = GameSave::from_ron(&saved.to_ron().unwrap()).unwrap();
        assert_eq!(saved.player.as_ref().unwrap().equipment[1].as_deref(), Some("stone_helmet"));
        let mut b = Arena::with_seed(seed);
        saved.apply(&mut b.game, &mut b.entities, &b.world, b.player);
        assert_eq!(saved.apply_items(&mut b.game, b.player, &b.defs, b.world.materials()), 4);
        b.tick(false, (1000, 0));
        assert_eq!(b.game.vitals.get(b.player).unwrap().armor, expected, "seed {seed}");
        assert_eq!((expected.defense, expected.fire, expected.cold), (18, 25, 10));
        // セット補正は保存された数値ではなく、現在の MOD の定義に従う。
        let mut replacement = b.defs.clone();
        replacement.layer(DefinitionFiles {
            items: Some(r#"[(name: "stone_helmet", replace: true, kind: Armor(slot: Head, defense: 4, set: Some("stone"), set_defense: 6))]"#),
            ..Default::default()
        }, b.world.materials(), Some("test")).unwrap();
        assert_ne!(replacement.content_hash(), b.defs.content_hash());
        let restored = b.game.equipment[&b.player].stats(|id| replacement.item_def(id));
        assert_eq!((restored.defense, restored.resist.cold), (20, 0));
        // セットのない古い定義と、未知の名前は引き続き読み飛ばす。
        let mut unknown = saved.clone();
        unknown.player.as_mut().unwrap().equipment[1] = Some("not_defined".to_owned());
        let mut c = Arena::with_seed(seed);
        assert_eq!(unknown.apply_items(&mut c.game, c.player, &c.defs, c.world.materials()), 3);
        c.tick(false, (1000, 0));
        assert_eq!(c.game.vitals.get(c.player).unwrap().armor.defense, 10);
    }
}

const MOD_WEAPONS: &str = r#"[
    (name: "bolt", kind: Misc),
    (name: "flame_blade", kind: Weapon(class: Melee, damage: 9, interval: 3, kind: Fire)),
    (name: "frost_bow", kind: Weapon(class: Ranged, damage: 14, interval: 4, kind: Cold, ammo: Some("bolt"))),
]"#;

/// 武器の定義ごとに間隔を変え、火の斬撃と冷気の矢は対応する耐性で軽減する。
#[test]
fn weapon_interval_and_damage_kind_follow_definition() {
    for seed in 1..=8 {
        let mut a = Arena::with_seed_and_items(seed, Some(MOD_WEAPONS));
        a.give_and_equip("m:flame_blade");
        let target = a.dummy(36);
        a.game.vitals.get_mut(target).unwrap().armor.fire = 50;
        a.game.take_events();
        a.tick(true, (1000, 0));
        assert_eq!(a.hp(target), 95, "seed {seed}: 火の耐性が 9 を 5 にする");
        for _ in 0..2 {
            a.tick(true, (1000, 0));
        }
        assert_eq!(
            a.game.take_events().iter().filter(|e| matches!(e, GameEvent::Melee { .. })).count(),
            1
        );
        a.tick(true, (1000, 0));
        assert_eq!(
            a.game.take_events().iter().filter(|e| matches!(e, GameEvent::Melee { .. })).count(),
            1,
            "seed {seed}: 3 tick 目に次の斬撃"
        );

        let mut b = Arena::with_seed_and_items(seed, Some(MOD_WEAPONS));
        b.give_and_equip("m:frost_bow");
        let target = b.dummy(36);
        b.game.vitals.get_mut(target).unwrap().armor.cold = 100;
        let ammo = b.defs.item_id("m:bolt", b.world.materials()).unwrap();
        b.game.inventories.get_mut(&b.player).unwrap().add_item(ammo, 2);
        b.tick(true, (1000, 0));
        let arrow = *b.game.projectiles.iter().next().unwrap();
        assert_eq!((arrow.damage, arrow.kind), (14, DamageKind::Cold));
        for _ in 0..5 {
            b.tick(false, (1000, 0));
        }
        assert!(
            b.game.take_events().iter().any(|event| matches!(event,
            GameEvent::Hit(hit) if hit.projectile == arrow.id && hit.entity == Some(target)
                && hit.damage == 14)),
            "seed {seed}: 冷気の矢が命中"
        );
        assert_eq!(b.hp(target), 100, "seed {seed}: 冷気の直撃は耐性 100% で無効");
        assert_eq!(b.game.vitals.get(target).unwrap().combat_immunity(), 0);
    }
}

/// 弾薬は発射が成立したときだけ 1 個消費し、失敗した入力・弾数上限では消費しない。
#[test]
fn ammunition_is_consumed_only_on_shot() {
    for seed in 1..=8 {
        let mut a = Arena::with_seed_and_items(seed, Some(MOD_WEAPONS));
        a.give_and_equip("m:frost_bow");
        let ammo = a.defs.item_id("m:bolt", a.world.materials()).unwrap();
        a.tick(true, (1000, 0));
        assert_eq!(a.game.projectiles.len(), 0, "seed {seed}: 未所持");
        a.game.inventories.get_mut(&a.player).unwrap().add_item(ammo, 2);
        a.tick(true, (0, 0));
        assert_eq!(a.game.inventories[&a.player].item_count(ammo), 2, "向きなし");
        assert_eq!(a.game.projectiles.len(), 0);
        a.tick(true, (1000, 0));
        assert_eq!(a.game.inventories[&a.player].item_count(ammo), 1);
        a.tick(true, (1000, 0));
        assert_eq!(a.game.inventories[&a.player].item_count(ammo), 1, "待ち時間");
        for _ in 0..4 {
            a.tick(false, (1000, 0));
        }
        a.tick(true, (1000, 0));
        assert_eq!(a.game.inventories[&a.player].item_count(ammo), 0);
        a.tick(true, (1000, 0));
        assert_eq!(a.game.inventories[&a.player].item_count(ammo), 0, "弾切れ");
        let mut full = Arena::with_seed_and_items(seed, Some(MOD_WEAPONS));
        full.give_and_equip("m:frost_bow");
        full.game.inventories.get_mut(&full.player).unwrap().add_item(ammo, 1);
        for _ in 0..charon_game::projectile::MAX_PROJECTILES {
            full.game
                .projectiles
                .spawn(&full.world, 0, 0, 0, 0, 0, 30, 0, charon_game::projectile::Payload::None)
                .unwrap();
        }
        full.tick(true, (1000, 0));
        assert_eq!(full.game.inventories[&full.player].item_count(ammo), 1, "上限でも消費しない");
        assert_eq!(full.game.projectiles.len(), charon_game::projectile::MAX_PROJECTILES);
    }
}

/// 押し続けたときの弾薬・弾 ID・世界とゲームのハッシュは seed ごとに再現できる。
#[test]
fn held_weapon_input_is_deterministic() {
    for seed in 1..=8 {
        let trace = || {
            let mut a = Arena::with_seed_and_items(seed, Some(MOD_WEAPONS));
            a.give_and_equip("m:frost_bow");
            let ammo = a.defs.item_id("m:bolt", a.world.materials()).unwrap();
            a.game.inventories.get_mut(&a.player).unwrap().add_item(ammo, 8);
            let mut trace = Vec::new();
            for _ in 0..30 {
                a.tick(true, (1000, 0));
                trace.push((
                    a.game.state_hash(),
                    a.world.state_hash(),
                    a.entities.state_hash(),
                    a.game.inventories[&a.player].item_count(ammo),
                ));
            }
            trace
        };
        assert_eq!(trace(), trace(), "seed {seed}");
    }
    let materials = MaterialTable::builtin().unwrap();
    let mut defs = Definitions::engine_defaults(&materials).unwrap();
    let original = defs.content_hash();
    for invalid in [
        r#"[(name: "invalid", kind: Weapon(class: Ranged, damage: 2, ammo: Some("missing")))]"#,
        r#"[(name: "invalid", kind: Weapon(class: Ranged, damage: 2, ammo: Some("wood_sword")))]"#,
        r#"[(name: "invalid", kind: Weapon(class: Melee, damage: 2, ammo: Some("wood_arrow")))]"#,
        r#"[(name: "invalid", kind: Weapon(class: Wand, wand: Some("basic"), interval: 2))]"#,
        r#"[(name: "invalid", kind: Weapon(class: Melee, damage: 2, interval: 601))]"#,
    ] {
        assert!(
            defs.layer(
                DefinitionFiles { items: Some(invalid), ..Default::default() },
                &materials,
                Some("m")
            )
            .is_err()
        );
        assert_eq!(defs.content_hash(), original, "失敗した層は変更しない");
    }
    defs.layer(
        DefinitionFiles { items: Some(MOD_WEAPONS), ..Default::default() },
        &materials,
        Some("m"),
    )
    .unwrap();
    let with_ammo = defs.content_hash();
    assert_ne!(with_ammo, original, "武器属性・間隔・弾薬が定義ハッシュに入る");
    let replacement = r#"[(name: "m:bolt", replace: true, kind: Weapon(class: Melee, damage: 1))]"#;
    assert!(
        defs.layer(
            DefinitionFiles { items: Some(replacement), ..Default::default() },
            &materials,
            None
        )
        .is_err()
    );
    assert_eq!(defs.content_hash(), with_ammo, "後続の上書きで弾薬を壊せない");
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
