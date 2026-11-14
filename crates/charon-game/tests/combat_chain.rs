//! 仕様 25 §6: 弾の着弾から相変化・戦利品・世界/装備の保存復帰まで。
#![allow(clippy::unwrap_used)] // 場面テストの前提が崩れたら失敗させる。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use charon_core::cell::MaterialId;
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::{DefinitionFiles, Definitions};
use charon_game::game::{GameInput, GameState};
use charon_game::items::EquipSlot;
use charon_game::projectile::{COLD_IMPACT_DELTA, Payload};
use charon_game::save::{GAME_SAVE_VERSION, GameSave};
use charon_game::status::StatusKind;
use charon_game::vitals::{DamageKind, MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld};
use charon_physics::PhysicsEngine;
use charon_world::region::RegionStore;
use charon_world::save::WorldMeta;

const ITEMS: &str = r#"[
    (name: "frost_bow", kind: Weapon(class: Ranged, damage: 16, kind: Cold, ammo: Some("wood_arrow"))),
]"#;
const ENEMIES: &str = r#"[
    (name: "target", width: 8, height: 16, max_hp: 40, walk_speed: 0,
     jump_speed: 0, sight: 1, attack: None, loot: [("stone", 3)]),
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
    fn new(seed: u64) -> Self {
        let materials = MaterialTable::builtin().unwrap();
        let mut defs = Definitions::engine_defaults(&materials).unwrap();
        defs.layer(
            DefinitionFiles { items: Some(ITEMS), enemies: Some(ENEMIES), ..Default::default() },
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
        Self { world, physics: PhysicsEngine::new(), entities, game, defs, player, tick: 0 }
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
        self.world.step();
        self.tick += 1;
    }

    fn cold_at(&mut self, x: i32, y: i32) {
        self.game
            .projectiles
            .spawn_kind(
                &self.world,
                i64::from(x - 1) * 16,
                i64::from(y) * 16,
                32,
                0,
                0,
                8,
                16,
                DamageKind::Cold,
                Payload::None,
            )
            .unwrap();
    }
}

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "charon-combat-chain-{}-{}",
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

/// 同じ入力と seed は冷気・火・爆風の着弾から敵の戦利品まで同じ順に進む。
#[test]
fn combat_environment_chain_is_deterministic() {
    for seed in 1..=8 {
        let run = || {
            let mut a = Arena::new(seed);
            let water = CellPos::new(34, 32);
            testkit::place(&mut a.world, water.x, water.y, 'w');
            a.cold_at(water.x, water.y);
            a.step();
            if a.world.get(water).material != testkit::material_of(&a.world, 'i') {
                a.world.step(); // parity が一致したセルは 1 tick 待ってから更新される。
            }
            assert_eq!(a.world.get(water).material, testkit::material_of(&a.world, 'i'));
            let ice_hash = a.world.state_hash();

            // 火の玉は木の手前に命中し、火/熱が次のセル更新へ進む。
            testkit::place(&mut a.world, 50, 32, 'W');
            let fire = testkit::material_of(&a.world, 'f');
            a.game
                .projectiles
                .spawn(
                    &a.world,
                    48 * 16,
                    32 * 16,
                    64,
                    0,
                    0,
                    8,
                    4,
                    Payload::Ignite { material: fire, radius: 2 },
                )
                .unwrap();
            a.step();
            let hot_wood = a.world.get(CellPos::new(50, 32));
            assert!(hot_wood.temp >= 300 || hot_wood.has(charon_core::cell::FLAG_BURNING));
            // セル更新後の火/熱は、次の GameState::step の環境判定で読む。
            a.entities.get_mut(a.player).unwrap().x16 = 32 * 16;
            a.step();
            assert!(
                a.game.vitals.get(a.player).unwrap().hp < MAX_HP,
                "seed {seed}: 火の環境ダメージ"
            );
            a.entities.get_mut(a.player).unwrap().x16 = 10 * 16;

            let target = a
                .game
                .spawn_enemy(&a.defs, &mut a.entities, &a.world, "test:target", (39, 26))
                .unwrap();
            a.game
                .projectiles
                .spawn(&a.world, 38 * 16, 32 * 16, 64, 0, 0, 8, 80, Payload::Explode { radius: 2 })
                .unwrap();
            a.step();
            assert!(a.entities.get(target).is_none(), "seed {seed}: 敵が倒れる");
            assert!(!a.game.drops.is_empty(), "seed {seed}: 戦利品が出る");
            (
                ice_hash,
                a.world.state_hash(),
                a.game.state_hash(),
                a.entities.state_hash(),
                a.game.drops.len(),
            )
        };
        assert_eq!(run(), run(), "seed {seed}: 戦闘とセルの固定 tick");
    }
}

/// 冷却は温度だけを変え、セルの数と未読込/空セルは変えず、静かな地形は眠る。
#[test]
fn combat_chain_keeps_cell_invariants() {
    for seed in 1..=8 {
        let mut a = Arena::new(seed);
        let at = CellPos::new(34, 32);
        testkit::place(&mut a.world, at.x, at.y, 'w');
        let before = a.world.get(at);
        let count = testkit::count(&a.world, 'w');
        a.cold_at(at.x, at.y);
        a.game
            .step(
                &mut a.world,
                &mut a.physics,
                &mut a.entities,
                &[(a.player, GameInput::default())],
                a.tick,
                seed,
            )
            .unwrap();
        assert_eq!(a.world.get(at).material, before.material);
        assert_eq!(a.world.get(at).temp, before.temp - COLD_IMPACT_DELTA);
        assert_eq!(testkit::count(&a.world, 'w'), count, "seed {seed}: 冷却だけでは消えない");
        a.world.step();
        if a.world.get(at).material != testkit::material_of(&a.world, 'i') {
            a.world.step(); // parity が一致したセルは 1 tick 待つ。
        }
        assert_eq!(a.world.get(at).material, testkit::material_of(&a.world, 'i'));
        assert_eq!(testkit::count(&a.world, 'w') + testkit::count(&a.world, 'i'), count);
        testkit::assert_sleeps(&mut a.world, 2000);
        let stable = a.world.state_hash();
        for _ in 0..4 {
            a.world.step();
            assert_eq!(a.world.state_hash(), stable, "seed {seed}: 休眠後は書かない");
        }
        // キャラクターへの命中は空セルを変えない。
        let target = a
            .game
            .spawn_enemy(&a.defs, &mut a.entities, &a.world, "test:target", (40, 26))
            .unwrap();
        let empty_pos = CellPos::new(40, 32);
        let empty = a.world.get(empty_pos);
        a.cold_at(empty_pos.x, empty_pos.y);
        a.game
            .step(
                &mut a.world,
                &mut a.physics,
                &mut a.entities,
                &[(a.player, GameInput::default())],
                a.tick,
                seed,
            )
            .unwrap();
        assert!(a.game.statuses.has(target, StatusKind::Slow));
        assert_eq!(a.world.get(empty_pos), empty);

        // 未読込の境界と剛体の旗が付いたセルは冷却しない。
        let boundary_pos = CellPos::new(64, 30);
        let boundary = a.world.get(boundary_pos);
        assert_eq!(boundary.material, MaterialId::BOUNDARY);
        a.cold_at(boundary_pos.x, boundary_pos.y);
        a.game
            .step(
                &mut a.world,
                &mut a.physics,
                &mut a.entities,
                &[(a.player, GameInput::default())],
                a.tick,
                seed,
            )
            .unwrap();
        assert_eq!(a.world.get(boundary_pos), boundary);
        let body_pos = CellPos::new(25, 30);
        testkit::place(&mut a.world, body_pos.x, body_pos.y, '#');
        let body = a.world.get(body_pos).with(charon_core::cell::FLAG_BODY, true);
        a.world.set(body_pos, body);
        a.cold_at(body_pos.x, body_pos.y);
        a.game
            .step(
                &mut a.world,
                &mut a.physics,
                &mut a.entities,
                &[(a.player, GameInput::default())],
                a.tick,
                seed,
            )
            .unwrap();
        assert_eq!(a.world.get(body_pos), body);
    }
}

/// セルはリージョン、プレイヤー装備と弾薬は game.ron の名前で復帰する。
#[test]
fn combat_loadout_survives_world_reload() {
    for seed in 1..=8 {
        let mut a = Arena::new(seed);
        let bow = a.defs.item_id("test:frost_bow", a.world.materials()).unwrap();
        let ammo = a.defs.item_id("wood_arrow", a.world.materials()).unwrap();
        let inv = a.game.inventories.get_mut(&a.player).unwrap();
        inv.add_item(bow, 1);
        inv.add_item(ammo, 3);
        a.game.equip(a.player, bow, &a.defs).unwrap();
        let water = CellPos::new(34, 32);
        testkit::place(&mut a.world, water.x, water.y, 'w');
        a.cold_at(water.x, water.y);
        a.step();
        if a.world.get(water).material != testkit::material_of(&a.world, 'i') {
            a.world.step();
        }
        assert_eq!(a.world.get(water).material, testkit::material_of(&a.world, 'i'));
        a.game.statuses.apply(a.player, StatusKind::Slow, 17);
        a.game.vitals.get_mut(a.player).unwrap().damage_combat(1, DamageKind::Physical);
        a.game
            .projectiles
            .spawn(&a.world, 20 * 16, 20 * 16, 16, 0, 0, 10, 1, Payload::None)
            .unwrap();

        let dir = TempDir::new();
        let store = RegionStore::new(dir.0.clone());
        let pos = ChunkPos::new(0, 0);
        store.write_chunk(pos, a.world.chunk(pos).unwrap(), a.world.materials()).unwrap();
        let meta = WorldMeta::new(seed, a.world.materials());
        let saved = GameSave::capture(&a.game, &a.entities, a.player, a.world.materials(), a.tick)
            .with_items(&a.game, a.player, &a.defs, a.world.materials());
        assert_eq!(saved.version, GAME_SAVE_VERSION);
        let saved = GameSave::from_ron(&saved.to_ron().unwrap()).unwrap();
        let loaded = store
            .read_chunk(pos, &meta.materials, Arc::new(MaterialTable::builtin().unwrap()))
            .unwrap()
            .unwrap();
        let mut b = Arena::new(seed);
        b.world.insert_loaded_chunk(pos, loaded);
        assert_eq!(b.world.get(water).material, a.world.get(water).material);
        saved.apply(&mut b.game, &mut b.entities, &b.world, b.player);
        assert_eq!(saved.apply_items(&mut b.game, b.player, &b.defs, b.world.materials()), 1);
        assert_eq!(b.game.equipment[&b.player].get(EquipSlot::Weapon), Some(bow));
        assert_eq!(b.game.inventories[&b.player].item_count(ammo), 3);
        assert_eq!(b.game.vitals.get(b.player).unwrap().combat_immunity(), 0);
        assert!(!b.game.statuses.has(b.player, StatusKind::Slow));
        assert_eq!(b.game.projectiles.len(), 0);
        // 復帰した弓は弾薬を 1 個使って、同じ属性の弾を撃てる。
        b.game
            .step(
                &mut b.world,
                &mut b.physics,
                &mut b.entities,
                &[(b.player, GameInput { fire: true, aim: (100, 0), ..Default::default() })],
                b.tick,
                seed,
            )
            .unwrap();
        assert_eq!(b.game.inventories[&b.player].item_count(ammo), 2);
    }
}
