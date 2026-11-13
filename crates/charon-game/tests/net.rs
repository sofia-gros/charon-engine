//! 仕様 13: 通信 (TCP のホスト中継 + Lockstep)。ローカルの 127.0.0.1 だけを使う。

#![allow(clippy::unwrap_used)] // テストの不変条件の失敗は即座に報告する。

use std::io::Write;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use charon_core::cell::Cell;
use charon_core::testkit::{box_art, material_of, world_from_ascii};
use charon_core::world::World;
use charon_game::game::{GameInput, GameState};
use charon_game::lockstep::{ConnectionInfo, Lockstep, TickInput};
use charon_game::mods::ModManifest;
use charon_game::net::{Client, Host, HostEvent, NetError, decode_info, encode_info};
use charon_game::vitals::Vitals;
use charon_game::wand::SpellTable;
use charon_game::{EntityId, EntityWorld, PlayerInput};
use charon_physics::PhysicsEngine;
use charon_world::background::BackgroundWorld;

struct Peer {
    world: World,
    entities: EntityWorld,
    physics: PhysicsEngine,
    background: BackgroundWorld,
    game: GameState,
    players: [EntityId; 2],
}

fn peer(seed: u64) -> Peer {
    let mut world = world_from_ascii(&box_art(62, 64), seed);
    let mut entities = EntityWorld::new();
    let a = entities.spawn_player(10 * 16, 0, &world).unwrap();
    let b = entities.spawn_player(30 * 16, 0, &world).unwrap();
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
    let mut game = GameState::new(SpellTable::default());
    for id in [a, b] {
        game.vitals.insert(id, Vitals::new(100).unwrap());
    }
    Peer { world, entities, physics, background: BackgroundWorld::new(), game, players: [a, b] }
}

fn info(world: &World) -> ConnectionInfo {
    ConnectionInfo::new(world, &[], "same-binary".to_owned(), "parallel".to_owned())
}

/// `condition` が真になるまで、`step` を呼びながら待つ (最大 5 秒)。
fn wait_until(mut step: impl FnMut() -> bool) {
    let start = Instant::now();
    while !step() {
        assert!(start.elapsed() < Duration::from_secs(5), "待ちきれなかった");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn input(tick: u64, player: EntityId, salt: u64) -> TickInput {
    let n = tick.wrapping_mul(2_654_435_761).wrapping_add(salt * 97);
    TickInput {
        tick,
        player,
        input: PlayerInput {
            horizontal: ((n >> 3) % 3) as i8 - 1,
            jump: (n >> 7).is_multiple_of(11),
        },
        action: GameInput::default(),
    }
}

/// 2 つの別々の世界が、TCP で配られた同じ束で、毎 tick 同じハッシュになる。
#[test]
fn two_peers_stay_in_sync_over_tcp() {
    for seed in 1..=3 {
        let (mut host_side, mut client_side) = (peer(seed), peer(seed));
        let identity = info(&host_side.world);
        let mut host = Host::listen(
            "127.0.0.1:0",
            host_side.players.to_vec(),
            identity.clone(),
            host_side.world.tick(),
        )
        .unwrap();
        let mut client = Client::connect(host.local_addr().unwrap(), &identity).unwrap();
        let mut joined = Vec::new();
        wait_until(|| {
            joined.extend(host.poll().unwrap());
            client.poll().unwrap();
            host.started() && client.started()
        });
        assert_eq!(joined, [HostEvent::Joined(1)]);
        assert_eq!(client.slot(), Some(1));
        assert_eq!(client.session(), Some((2, 0)));

        let mut lock_host = Lockstep::new(
            &host_side.players,
            &host_side.world,
            &host_side.entities,
            None,
            identity.clone(),
        )
        .unwrap();
        let mut lock_client = Lockstep::new(
            &client_side.players,
            &client_side.world,
            &client_side.entities,
            None,
            identity.clone(),
        )
        .unwrap();
        for id in host_side.players {
            lock_host.connect(id, &identity).unwrap();
            lock_client.connect(id, &identity).unwrap();
        }
        let mut last = None;
        for tick in 0..90 {
            host.submit_local(input(tick, host_side.players[0], 1)).unwrap();
            client.send_input(input(tick, client_side.players[1], 2));
            let (mut from_host, mut from_client) = (None, None);
            wait_until(|| {
                host.poll().unwrap();
                client.poll().unwrap();
                from_host = from_host.take().or_else(|| host.take_bundle());
                from_client = from_client.take().or_else(|| client.take_bundle());
                from_host.is_some() && from_client.is_some()
            });
            let (bundle_host, bundle_client) = (from_host.unwrap(), from_client.unwrap());
            assert_eq!(bundle_host, bundle_client, "seed {seed}, tick {tick}: 同じ束が配られる");
            assert_eq!(bundle_host.len(), 2);
            assert!(bundle_host[0].player < bundle_host[1].player, "ID の昇順");
            for packet in &bundle_host {
                lock_host.submit(*packet).unwrap();
                lock_client.submit(*packet).unwrap();
            }
            let a = lock_host
                .advance(
                    &mut host_side.world,
                    &mut host_side.entities,
                    &mut host_side.physics,
                    &host_side.background,
                    &mut host_side.game,
                )
                .unwrap()
                .unwrap();
            let b = lock_client
                .advance(
                    &mut client_side.world,
                    &mut client_side.entities,
                    &mut client_side.physics,
                    &client_side.background,
                    &mut client_side.game,
                )
                .unwrap()
                .unwrap();
            assert_eq!(a, b, "seed {seed}, tick {tick}: ハッシュが一致する");
            last = Some(a);
        }
        // 入力に意味があった (プレイヤーが動いた) ことを確かめる。
        let moved = host_side.entities.get(host_side.players[0]).unwrap();
        assert_ne!(moved.x16, 10 * 16, "seed {seed}: プレイヤーが動いた");
        assert!(last.is_some());
    }
}

/// 接続情報が違う相手は、ホストに断られる。
#[test]
fn mismatched_peers_are_refused() {
    let side = peer(1);
    let identity = info(&side.world);
    let mut host = Host::listen("127.0.0.1:0", side.players.to_vec(), identity.clone(), 0).unwrap();
    let other = {
        let different = peer(2); // seed が違う
        info(&different.world)
    };
    let mut client = Client::connect(host.local_addr().unwrap(), &other).unwrap();
    let mut events = Vec::new();
    let mut result = Ok(());
    wait_until(|| {
        events.extend(host.poll().unwrap());
        result = client.poll();
        result.is_err()
    });
    assert!(matches!(result, Err(NetError::Rejected(_))), "{result:?}");
    assert!(matches!(events.as_slice(), [HostEvent::Refused(_)]), "{events:?}");
    assert!(!host.started());
    assert_eq!(client.slot(), None);
    // ビルドが違っても断られる。
    let mut other = identity.clone();
    other.binary = "another-build".to_owned();
    let mut client = Client::connect(host.local_addr().unwrap(), &other).unwrap();
    wait_until(|| {
        host.poll().unwrap();
        client.poll().is_err()
    });
}

/// 席が埋まったあとの接続は断られ、他人のふりをした入力・同じ tick の 2 度目・先すぎる tick は、接続を切られる。
#[test]
fn bad_inputs_drop_the_connection() {
    let side = peer(1);
    let identity = info(&side.world);
    let setup = |host: &mut Host| {
        let mut client = Client::connect(host.local_addr().unwrap(), &identity).unwrap();
        wait_until(|| {
            host.poll().unwrap();
            client.poll().unwrap();
            host.started() && client.started()
        });
        client
    };
    // (1) 別の参加者 (ホスト自身) のふり。
    let mut host = Host::listen("127.0.0.1:0", side.players.to_vec(), identity.clone(), 0).unwrap();
    let mut client = setup(&mut host);
    client.send_input(input(0, side.players[0], 1));
    let mut events = Vec::new();
    wait_until(|| {
        events.extend(host.poll().unwrap());
        events.contains(&HostEvent::Left(1))
    });
    assert!(matches!(wait_error(&mut client), NetError::Disconnected | NetError::Io(_)));
    // (2) 同じ tick の入力を 2 度。
    let mut host = Host::listen("127.0.0.1:0", side.players.to_vec(), identity.clone(), 0).unwrap();
    let mut client = setup(&mut host);
    client.send_input(input(0, side.players[1], 2));
    client.send_input(input(0, side.players[1], 3));
    let mut events = Vec::new();
    wait_until(|| {
        events.extend(host.poll().unwrap());
        events.contains(&HostEvent::Left(1))
    });
    // (3) 先すぎる tick。
    let mut host = Host::listen("127.0.0.1:0", side.players.to_vec(), identity.clone(), 0).unwrap();
    let mut client = setup(&mut host);
    client.send_input(input(100_000, side.players[1], 4));
    let mut events = Vec::new();
    wait_until(|| {
        events.extend(host.poll().unwrap());
        events.contains(&HostEvent::Left(1))
    });
    // (4) ホスト自身の入力の検査。
    let mut host = Host::listen("127.0.0.1:0", side.players.to_vec(), identity, 0).unwrap();
    assert!(host.submit_local(input(0, side.players[1], 1)).is_err(), "ホストの入力ではない");
    assert!(host.submit_local(input(100_000, side.players[0], 1)).is_err(), "先すぎる");
    assert!(host.submit_local(input(0, side.players[0], 1)).is_ok());
    assert!(host.submit_local(input(0, side.players[0], 1)).is_err(), "2 度目");
    assert!(host.take_bundle().is_none(), "始まる前は束を作らない");
}

fn wait_error(client: &mut Client) -> NetError {
    let start = Instant::now();
    loop {
        if let Err(e) = client.poll() {
            return e;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "切れなかった");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// 大きすぎるメッセージ・長さ 0・知らない種類は、接続を切る。
#[test]
fn malformed_frames_are_dropped() {
    let side = peer(1);
    let identity = info(&side.world);
    for frame in [
        (u32::MAX).to_le_bytes().to_vec(),
        (2_000_000u32).to_le_bytes().to_vec(),
        0u32.to_le_bytes().to_vec(),
        [2u32.to_le_bytes().to_vec(), vec![99, 0]].concat(),
    ] {
        let mut host =
            Host::listen("127.0.0.1:0", side.players.to_vec(), identity.clone(), 0).unwrap();
        let mut raw = TcpStream::connect(host.local_addr().unwrap()).unwrap();
        raw.write_all(&frame).unwrap();
        raw.flush().unwrap();
        // 接続を受け付けたあと、切って待ち受けに戻る (落ちない)。
        for _ in 0..200 {
            host.poll().unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!host.started());
        // その後も、正しい相手はつながる。
        let mut client = Client::connect(host.local_addr().unwrap(), &identity).unwrap();
        wait_until(|| {
            host.poll().unwrap();
            client.poll().unwrap();
            host.started() && client.started()
        });
    }
}

/// 接続情報の符号化: 往復で一致し、途中で切れたもの・余りがあるものは拒否する。
#[test]
fn connection_info_roundtrips_and_rejects_garbage() {
    let side = peer(1);
    let mut identity = info(&side.world);
    identity.mods = vec![ModManifest { id: "fse_demo".to_owned(), version: 3 }];
    identity.definitions = 0xDEAD_BEEF;
    let bytes = encode_info(&identity);
    assert_eq!(decode_info(&bytes).unwrap(), identity);
    for cut in [0, 1, 5, bytes.len() / 2, bytes.len() - 1] {
        assert!(decode_info(&bytes[..cut]).is_err(), "{cut} バイトで切れている");
    }
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(decode_info(&extra).is_err(), "余りがある");
    // 巨大な個数を名乗っても、メモリを確保せずに拒否する。
    let mut huge = bytes;
    huge[18..22].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode_info(&huge).is_err());
}
