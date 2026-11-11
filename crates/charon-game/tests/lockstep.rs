#![allow(clippy::unwrap_used)] // テストの不変条件の失敗は即座に報告する。

use charon_core::cell::Cell;
use charon_core::coords::CellPos;
use charon_core::testkit::{box_art, material_of, world_from_ascii};
use charon_core::world::World;
use charon_game::lockstep::{
    ConnectionInfo, Lockstep, LockstepError, TickInput, synchronization_hash,
};
use charon_game::mods::{ModManifest, ModPack};
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
        let mut lockstep = connected(&world, &entities, players);
        let before = (world.state_hash(), entities.state_hash(), world.tick());
        lockstep
            .submit(TickInput {
                tick: 0,
                player: players[0],
                input: PlayerInput { horizontal: 1, jump: false },
            })
            .unwrap();
        assert!(
            lockstep
                .advance(&mut world, &mut entities, &mut physics, &background)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            (world.state_hash(), entities.state_hash(), world.tick()),
            before,
            "seed={seed}"
        );
        lockstep
            .submit(TickInput { tick: 0, player: players[1], input: PlayerInput::default() })
            .unwrap();
        let result =
            lockstep.advance(&mut world, &mut entities, &mut physics, &background).unwrap();
        assert_eq!(
            result,
            Some(synchronization_hash(&world, &entities, &physics, &background).unwrap())
        );
        assert_eq!(world.tick(), 1, "seed={seed}");
    }
}

fn trace(seed: u64, reversed: bool) -> Vec<u64> {
    let (mut world, mut entities, mut physics, mut background, players) = scene(seed);
    let mut lockstep = connected(&world, &entities, players);
    background.set(CellPos::new(-1, 0), material_of(&world, '#'));
    let mut hashes = Vec::new();
    for tick in 0..20 {
        for index in if reversed { [1, 0] } else { [0, 1] } {
            let packet = TickInput {
                tick,
                player: players[index],
                input: PlayerInput { horizontal: if index == 0 { 1 } else { -1 }, jump: false },
            }
            .encode();
            lockstep.submit_bytes(&packet).unwrap();
        }
        hashes.push(
            lockstep
                .advance(&mut world, &mut entities, &mut physics, &background)
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
        let info =
            ConnectionInfo::new(&world, &[], "build-one".to_string(), "parallel".to_string());
        let mut lockstep = Lockstep::new(&players, &world, &entities, None, info.clone()).unwrap();
        let mut changed = info.clone();
        changed.version = 2;
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
            tick: 0,
            player: players[0],
            input: PlayerInput { horizontal: 1, jump: false },
        };
        let before = (world.tick(), world.state_hash(), entities.state_hash());
        let packet = input.encode();
        assert_eq!(&packet[..2], &[1, 0]);
        assert_eq!(&packet[2..10], &0u64.to_le_bytes());
        assert_eq!(&packet[10..18], &players[0].0.to_le_bytes());
        assert_eq!(TickInput::decode(&packet).unwrap(), input);
        let mut overflow_tick = packet;
        overflow_tick[2..10].fill(0xFF);
        assert!(matches!(TickInput::decode(&overflow_tick), Err(LockstepError::InvalidPacket)));
        for invalid in [&packet[..19], &[0, 0, 0][..]] {
            assert!(matches!(lockstep.submit_bytes(invalid), Err(LockstepError::InvalidPacket)));
        }
        for index in [0, 10, 18, 19] {
            let mut invalid = packet;
            invalid[index] = if index == 0 {
                2
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
                .advance(&mut world, &mut entities, &mut physics, &background)
                .unwrap()
                .is_none()
        );
        assert_eq!((world.tick(), world.state_hash(), entities.state_hash()), before);
        lockstep
            .submit(TickInput { tick: 0, player: players[1], input: PlayerInput::default() })
            .unwrap();
        lockstep.advance(&mut world, &mut entities, &mut physics, &background).unwrap().unwrap();
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
    let initial = synchronization_hash(&world, &entities, &physics, &background).unwrap();
    background.set(CellPos::new(2, 2), material_of(&world, '#'));
    assert_ne!(initial, synchronization_hash(&world, &entities, &physics, &background).unwrap());
    background.set(CellPos::new(2, 2), charon_core::cell::MaterialId::EMPTY);
    let original = synchronization_hash(&world, &entities, &physics, &background).unwrap();
    physics.drag_body(0, [48.0, 28.0]);
    assert_ne!(original, synchronization_hash(&world, &entities, &physics, &background).unwrap());
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
                tick: 0,
                player,
                input: PlayerInput { horizontal: 1, jump: false },
            })
            .unwrap();
        lockstep
            .advance(&mut world, &mut entities, &mut PhysicsEngine::new(), &BackgroundWorld::new())
            .unwrap()
            .unwrap();
        assert_ne!(world.get(CellPos::new(18, 8)).material, before, "seed={seed}");
        assert_eq!(entities.get(player).unwrap().x16, 12 * 16, "seed={seed}");
        fs::remove_dir_all(path).unwrap();
    }
}
