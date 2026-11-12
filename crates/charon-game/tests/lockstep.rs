#![allow(clippy::unwrap_used)] // テストの不変条件の失敗は即座に報告する。

use charon_core::cell::Cell;
use charon_core::coords::CellPos;
use charon_core::testkit::{box_art, material_of, world_from_ascii};
use charon_core::world::World;
use charon_game::game::{GameInput, GameState};
use charon_game::lockstep::{
    ConnectionInfo, Lockstep, LockstepError, TickInput, synchronization_hash,
};
use charon_game::mods::{ModManifest, ModPack};
use charon_game::vitals::Vitals;
use charon_game::wand::{SpellId, SpellTable, Wand};
use charon_game::{EntityId, EntityWorld, PlayerInput};
use charon_physics::PhysicsEngine;
use charon_world::background::BackgroundWorld;

fn scene(seed: u64) -> (World, EntityWorld, PhysicsEngine, BackgroundWorld, [EntityId; 2]) {
    let mut world = world_from_ascii(&box_art(62, 64), seed);
    let mut entities = EntityWorld::new();
    let a = entities.spawn_player(10 * 16, 8 * 16, &world).unwrap();
    let b = entities.spawn_player(30 * 16, 8 * 16, &world).unwrap();
    let stone = material_of(&world, '#');
    let mut physics = PhysicsEngine::new();
    physics
        .create_body(
            2,
            2,
            vec![Cell { material: stone, ..Cell::default() }; 4],
            world.materials(),
            [47.0, 28.0],
        )
        .unwrap();
    physics.stamp_all(&mut world);
    (world, entities, physics, BackgroundWorld::new(), [a, b])
}

fn connected(world: &World, entities: &EntityWorld, players: [EntityId; 2]) -> Lockstep<'static> {
    let info = ConnectionInfo::new(world, &[], "same-binary".to_string(), "parallel".to_string());
    let mut lockstep =
        Lockstep::new(&[players[1], players[0]], world, entities, None, info.clone()).unwrap();
    for player in players {
        lockstep.connect(player, &info).unwrap();
    }
    lockstep
}

#[test]
fn lockstep_waits_for_all_inputs() {
    for seed in 1..=8 {
        let (mut world, mut entities, mut physics, background, players) = scene(seed);
        let mut game = GameState::new(SpellTable::default());
        let mut lockstep = connected(&world, &entities, players);
        let before = (world.state_hash(), entities.state_hash(), world.tick());
        lockstep
            .submit(TickInput {
                action: GameInput::default(),
                tick: 0,
                player: players[0],
                input: PlayerInput { horizontal: 1, jump: false },
            })
            .unwrap();
        assert!(
            lockstep
                .advance(&mut world, &mut entities, &mut physics, &background, &mut game)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            (world.state_hash(), entities.state_hash(), world.tick()),
            before,
            "seed={seed}"
        );
        lockstep
            .submit(TickInput {
                action: GameInput::default(),
                tick: 0,
                player: players[1],
                input: PlayerInput::default(),
            })
            .unwrap();
        let result = lockstep
            .advance(&mut world, &mut entities, &mut physics, &background, &mut game)
            .unwrap();
        assert_eq!(
            result,
            Some(synchronization_hash(&world, &entities, &physics, &background, &game).unwrap())
        );
        assert_eq!(world.tick(), 1, "seed={seed}");
    }
}

fn trace(seed: u64, reversed: bool) -> Vec<u64> {
    let (mut world, mut entities, mut physics, mut background, players) = scene(seed);
    let mut game = GameState::new(SpellTable::default());
    let mut lockstep = connected(&world, &entities, players);
    background.set(CellPos::new(-1, 0), material_of(&world, '#'));
    let mut hashes = Vec::new();
    for tick in 0..20 {
        for index in if reversed { [1, 0] } else { [0, 1] } {
            let packet = TickInput {
                action: GameInput::default(),
                tick,
                player: players[index],
                input: PlayerInput { horizontal: if index == 0 { 1 } else { -1 }, jump: false },
            }
            .encode();
            lockstep.submit_bytes(&packet).unwrap();
        }
        hashes.push(
            lockstep
                .advance(&mut world, &mut entities, &mut physics, &background, &mut game)
                .unwrap()
                .unwrap(),
        );
    }
    hashes
}

#[test]
fn lockstep_order_is_stable() {
    for seed in 1..=8 {
        assert_eq!(trace(seed, false), trace(seed, true), "seed={seed}");
    }
}

#[test]
fn lockstep_rejects_mismatch() {
    for seed in 1..=8 {
        let (mut world, mut entities, mut physics, background, players) = scene(seed);
        let mut game = GameState::new(SpellTable::default());
        let info =
            ConnectionInfo::new(&world, &[], "build-one".to_string(), "parallel".to_string());
        let mut lockstep = Lockstep::new(&players, &world, &entities, None, info.clone()).unwrap();
        let mut changed = info.clone();
        changed.version = 3;
        assert!(matches!(lockstep.connect(players[0], &changed), Err(LockstepError::Mismatch)));
        changed = info.clone();
        changed.materials.swap(2, 3);
        assert!(matches!(lockstep.connect(players[0], &changed), Err(LockstepError::Mismatch)));
        changed = info.clone();
        changed.mods.push(ModManifest { id: "extra".to_string(), version: 1 });
        assert!(matches!(lockstep.connect(players[0], &changed), Err(LockstepError::Mismatch)));
        changed = info.clone();
        changed.seed += 1;
        assert!(matches!(lockstep.connect(players[0], &changed), Err(LockstepError::Mismatch)));
        changed = info.clone();
        changed.binary = "build-two".to_string();
        assert!(matches!(lockstep.connect(players[0], &changed), Err(LockstepError::Mismatch)));
        changed = info.clone();
        changed.features = "serial".to_string();
        assert!(matches!(lockstep.connect(players[0], &changed), Err(LockstepError::Mismatch)));
        changed = info.clone();
        changed.platform = "another-os/arch".to_string();
        assert!(matches!(lockstep.connect(players[0], &changed), Err(LockstepError::Mismatch)));
        lockstep.connect(players[0], &info).unwrap();
        lockstep.connect(players[1], &info).unwrap();
        let input = TickInput {
            action: GameInput::default(),
            tick: 0,
            player: players[0],
            input: PlayerInput { horizontal: 1, jump: false },
        };
        let before = (world.tick(), world.state_hash(), entities.state_hash());
        let packet = input.encode();
        assert_eq!(&packet[..2], &[2, 0]);
        assert_eq!(&packet[2..10], &0u64.to_le_bytes());
        assert_eq!(&packet[10..18], &players[0].0.to_le_bytes());
        assert_eq!(TickInput::decode(&packet).unwrap(), input);
        let mut overflow_tick = packet;
        overflow_tick[2..10].fill(0xFF);
        assert!(matches!(TickInput::decode(&overflow_tick), Err(LockstepError::InvalidPacket)));
        for invalid in [&packet[..19], &[0, 0, 0][..]] {
            assert!(matches!(lockstep.submit_bytes(invalid), Err(LockstepError::InvalidPacket)));
        }
        for index in [0, 10, 18, 19, 20] {
            let mut invalid = packet;
            invalid[index] = if index == 0 {
                1
            } else if index == 10 {
                0
            } else {
                3
            };
            if index == 10 {
                invalid[11..18].fill(0);
            }
            assert!(lockstep.submit_bytes(&invalid).is_err(), "seed={seed}, index={index}");
        }
        lockstep.submit(input).unwrap();
        let conflicting = TickInput { input: PlayerInput { horizontal: -1, jump: false }, ..input };
        assert!(matches!(lockstep.submit(conflicting), Err(LockstepError::DuplicateInput)));
        assert!(matches!(
            lockstep.submit(TickInput { tick: 1, ..input }),
            Err(LockstepError::WrongTick)
        ));
        assert!(
            lockstep
                .advance(&mut world, &mut entities, &mut physics, &background, &mut game)
                .unwrap()
                .is_none()
        );
        assert_eq!((world.tick(), world.state_hash(), entities.state_hash()), before);
        lockstep
            .submit(TickInput {
                action: GameInput::default(),
                tick: 0,
                player: players[1],
                input: PlayerInput::default(),
            })
            .unwrap();
        lockstep
            .advance(&mut world, &mut entities, &mut physics, &background, &mut game)
            .unwrap()
            .unwrap();
        assert_eq!(
            entities.get(players[0]).unwrap().x16,
            12 * 16,
            "seed={seed}: 重複入力に上書きされた"
        );
        assert!(matches!(lockstep.submit(input), Err(LockstepError::WrongTick)));
    }
}

#[test]
fn synchronization_hash_includes_body_and_background() {
    let (world, entities, mut physics, mut background, _) = scene(1);
    let game = GameState::new(SpellTable::default());
    let initial = synchronization_hash(&world, &entities, &physics, &background, &game).unwrap();
    background.set(CellPos::new(2, 2), material_of(&world, '#'));
    assert_ne!(
        initial,
        synchronization_hash(&world, &entities, &physics, &background, &game).unwrap()
    );
    background.set(CellPos::new(2, 2), charon_core::cell::MaterialId::EMPTY);
    let original = synchronization_hash(&world, &entities, &physics, &background, &game).unwrap();
    physics.drag_body(0, [48.0, 28.0]);
    assert_ne!(
        original,
        synchronization_hash(&world, &entities, &physics, &background, &game).unwrap()
    );
}

#[test]
fn script_precedes_player_collision() {
    use charon_core::coords::ChunkPos;
    use charon_core::testkit::fill;
    use charon_game::mods::ModDirectory;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    for seed in 1..=8 {
        let path = std::env::temp_dir().join(format!(
            "charon-lockstep-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("manifest.ron"), "(id: \"unblock\", version: 1)").unwrap();
        fs::write(path.join("materials.ron"), "[]").unwrap();
        fs::write(path.join("reactions.ron"), "[]").unwrap();
        fs::write(
            path.join("script.ron"),
            r#"[(every: 1, x: 18, y: 8, from: "stone", into: "empty")]"#,
        )
        .unwrap();
        let pack = ModPack::load(&[ModDirectory { path: path.clone(), script: true }]).unwrap();
        let mut world = pack.new_world(seed);
        world.insert_chunk(ChunkPos::new(0, 0));
        fill(&mut world, 18, 8, 18, 8, '#');
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(10 * 16, 8 * 16, &world).unwrap();
        let mut game = GameState::new(SpellTable::default());
        let before = world.get(CellPos::new(18, 8)).material;
        let info = ConnectionInfo::new(
            &world,
            &pack.manifests,
            "build".to_string(),
            "parallel".to_string(),
        );
        let mut lockstep =
            Lockstep::new(&[player], &world, &entities, Some(&pack), info.clone()).unwrap();
        lockstep.connect(player, &info).unwrap();
        lockstep
            .submit(TickInput {
                action: GameInput::default(),
                tick: 0,
                player,
                input: PlayerInput { horizontal: 1, jump: false },
            })
            .unwrap();
        lockstep
            .advance(
                &mut world,
                &mut entities,
                &mut PhysicsEngine::new(),
                &BackgroundWorld::new(),
                &mut game,
            )
            .unwrap()
            .unwrap();
        assert_ne!(world.get(CellPos::new(18, 8)).material, before, "seed={seed}");
        assert_eq!(entities.get(player).unwrap().x16, 12 * 16, "seed={seed}");
        fs::remove_dir_all(path).unwrap();
    }
}

const SPELLS: &str = r#"[
    (name: "spark", mana_cost: 5, cast_delay: 6, speed: 128, gravity: 0, life: 60,
     damage: 10, spread: 0, payload: None),
]"#;

fn spell_table(world: &World) -> SpellTable {
    SpellTable::parse(SPELLS, world.materials(), None).unwrap()
}

#[test]
fn lockstep_packet_v2_roundtrip() {
    let input = TickInput {
        tick: 7,
        player: EntityId(3),
        input: PlayerInput { horizontal: -1, jump: true },
        action: GameInput { fire: true, aim: (-5, 70_000) },
    };
    let packet = input.encode();
    assert_eq!(packet.len(), 29);
    assert_eq!(&packet[..2], &[2, 0]);
    assert_eq!(packet[20], 1);
    assert_eq!(&packet[21..25], &(-5i32).to_le_bytes());
    assert_eq!(&packet[25..29], &70_000i32.to_le_bytes());
    assert_eq!(TickInput::decode(&packet).unwrap(), input);
    assert_eq!(
        TickInput::decode(&TickInput { action: GameInput::default(), ..input }.encode())
            .unwrap()
            .action,
        GameInput::default()
    );

    let mut bad_fire = packet;
    bad_fire[20] = 2;
    assert!(matches!(TickInput::decode(&bad_fire), Err(LockstepError::InvalidPacket)));
    for offset in [21, 25] {
        let mut huge = packet;
        huge[offset..offset + 4].copy_from_slice(&1_000_001i32.to_le_bytes());
        assert!(matches!(TickInput::decode(&huge), Err(LockstepError::InvalidPacket)));
        let mut min = packet;
        min[offset..offset + 4].copy_from_slice(&i32::MIN.to_le_bytes());
        assert!(matches!(TickInput::decode(&min), Err(LockstepError::InvalidPacket)));
        let mut edge = packet;
        edge[offset..offset + 4].copy_from_slice(&(-1_000_000i32).to_le_bytes());
        assert!(TickInput::decode(&edge).is_ok(), "上限ちょうどは通る");
    }
    let mut version1 = packet;
    version1[..2].copy_from_slice(&1u16.to_le_bytes());
    assert!(matches!(TickInput::decode(&version1), Err(LockstepError::Mismatch)));
    assert!(matches!(TickInput::decode(&packet[..20]), Err(LockstepError::InvalidPacket)));
}

fn game_trace(seed: u64, reversed: bool) -> Vec<u64> {
    let (mut world, mut entities, mut physics, background, players) = scene(seed);
    let table = spell_table(&world);
    let mut game = GameState::new(table.clone());
    for id in players {
        let wand = Wand::new(vec![SpellId(0)], 100, 1, 0, &table).unwrap();
        game.add_character(id, Vitals::new(100).unwrap(), wand);
    }
    let info = ConnectionInfo::new(&world, &[], "same-binary".to_string(), "parallel".to_string())
        .with_spells(&table);
    let mut lockstep = Lockstep::new(&players, &world, &entities, None, info.clone()).unwrap();
    for player in players {
        lockstep.connect(player, &info).unwrap();
    }
    let mut hashes = Vec::new();
    for tick in 0..30 {
        for index in if reversed { [1, 0] } else { [0, 1] } {
            let fire = tick % 2 == 0 || index == 1;
            let aim = if index == 0 { (1, 0) } else { (-1, 0) };
            let packet = TickInput {
                tick,
                player: players[index],
                input: PlayerInput::default(),
                action: GameInput { fire, aim },
            }
            .encode();
            lockstep.submit_bytes(&packet).unwrap();
        }
        hashes.push(
            lockstep
                .advance(&mut world, &mut entities, &mut physics, &background, &mut game)
                .unwrap()
                .unwrap(),
        );
    }
    assert!(game.vitals.iter().any(|(_, v)| v.hp < 100), "弾が当たって体力が減る seed={seed}");
    hashes
}

#[test]
fn lockstep_game_hash_is_order_independent() {
    for seed in 1..=8 {
        assert_eq!(game_trace(seed, false), game_trace(seed, true), "seed={seed}");
    }

    // 呪文表が違う接続は拒否される。
    let (world, entities, _, _, players) = scene(1);
    let table = spell_table(&world);
    let with =
        ConnectionInfo::new(&world, &[], "b".to_string(), "p".to_string()).with_spells(&table);
    let without = ConnectionInfo::new(&world, &[], "b".to_string(), "p".to_string());
    let mut lockstep = Lockstep::new(&players, &world, &entities, None, with.clone()).unwrap();
    assert!(matches!(lockstep.connect(players[0], &without), Err(LockstepError::Mismatch)));
    lockstep.connect(players[0], &with).unwrap();

    // 杖を持たない参加者が撃とうとすると、何も更新せずに拒否される。
    let (mut world, mut entities, mut physics, background, players) = scene(1);
    let mut game = GameState::new(SpellTable::default());
    let info = ConnectionInfo::new(&world, &[], "b".to_string(), "p".to_string());
    let mut lockstep = Lockstep::new(&players, &world, &entities, None, info.clone()).unwrap();
    for player in players {
        lockstep.connect(player, &info).unwrap();
        lockstep
            .submit(TickInput {
                tick: 0,
                player,
                input: PlayerInput::default(),
                action: GameInput { fire: player == players[0], aim: (1, 0) },
            })
            .unwrap();
    }
    let before = (world.tick(), world.state_hash(), entities.state_hash());
    let err = lockstep
        .advance(&mut world, &mut entities, &mut physics, &background, &mut game)
        .unwrap_err();
    assert!(matches!(err, LockstepError::Game(charon_game::game::GameError::InvalidAction)));
    assert_eq!((world.tick(), world.state_hash(), entities.state_hash()), before);
}
