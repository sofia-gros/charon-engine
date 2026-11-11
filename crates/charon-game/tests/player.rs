use std::sync::Arc;

use charon_core::cell::FLAG_BODY;
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::{EntityError, EntityId, EntityWorld, PlayerInput, step_player};

fn scene(seed: u64, negative: bool) -> World {
    let mut world =
        World::new(Arc::new(MaterialTable::builtin().expect("組み込み材質を読み込む")), seed);
    world.insert_chunk(ChunkPos::new(0, 0));
    if negative {
        world.insert_chunk(ChunkPos::new(-1, 0));
        testkit::fill(&mut world, -64, 40, 63, 40, '#');
    } else {
        testkit::fill(&mut world, 0, 40, 63, 40, '#');
    }
    world
}

#[test]
fn player_lands_on_solid() {
    for seed in 1..=8 {
        let world = scene(seed, false);
        let mut entities = EntityWorld::new();
        let id = entities.spawn_player(8 * 16, 5 * 16, &world).unwrap();
        for _ in 0..100 {
            entities.step_players(&[], &world).unwrap();
        }
        let player = entities.get(id).unwrap();
        assert_eq!(player.y16, 24 * 16, "seed={seed}");
        assert!(player.grounded, "seed={seed}");
        assert_eq!(player.vy, 0);
        assert_eq!(world.get(CellPos::new(8, 40)).material, testkit::material_of(&world, '#'));
    }
}

#[test]
fn player_hits_body_stamp() {
    for seed in 1..=8 {
        let mut world = scene(seed, false);
        testkit::fill(&mut world, 18, 24, 18, 39, 'w');
        for y in 24..40 {
            let pos = CellPos::new(18, y);
            world.set(pos, world.get(pos).with(FLAG_BODY, true));
        }
        let mut entities = EntityWorld::new();
        let id = entities.spawn_player(8 * 16, 24 * 16, &world).unwrap();
        for _ in 0..8 {
            entities
                .step_players(&[(id, PlayerInput { horizontal: 1, jump: false })], &world)
                .unwrap();
        }
        assert_eq!(entities.get(id).unwrap().x16, 10 * 16, "seed={seed}");
        for y in 24..40 {
            let pos = CellPos::new(18, y);
            world.set(pos, world.get(pos).with(FLAG_BODY, false));
        }
        for _ in 0..8 {
            entities
                .step_players(&[(id, PlayerInput { horizontal: 1, jump: false })], &world)
                .unwrap();
        }
        assert!(entities.get(id).unwrap().x16 > 18 * 16, "seed={seed}");
    }
}

#[test]
fn player_jump_and_negative_coords() {
    for seed in 1..=8 {
        let mut world = scene(seed, true);
        testkit::fill(&mut world, -20, 24, -20, 39, '#');
        let mut entities = EntityWorld::new();
        let id = entities.spawn_player(-32 * 16, 24 * 16, &world).unwrap();
        entities.step_players(&[], &world).unwrap();
        assert!(entities.get(id).unwrap().grounded);
        for _ in 0..10 {
            entities
                .step_players(&[(id, PlayerInput { horizontal: 1, jump: false })], &world)
                .unwrap();
        }
        assert_eq!(entities.get(id).unwrap().x16, -28 * 16, "seed={seed}");
        entities.step_players(&[(id, PlayerInput { horizontal: 0, jump: true })], &world).unwrap();
        assert!(entities.get(id).unwrap().y16 < 24 * 16, "seed={seed}");
        for _ in 0..100 {
            entities.step_players(&[], &world).unwrap();
        }
        assert_eq!(entities.get(id).unwrap().y16, 24 * 16, "seed={seed}");
        assert!(entities.get(id).unwrap().grounded);
        // 負座標のセル境界を 1/16 セルだけ横切ったときも衝突する。
        let mut player = entities.get(id).unwrap().clone();
        player.x16 = -28 * 16 - 1;
        step_player(&mut player, PlayerInput { horizontal: 1, jump: false }, &world);
        assert_eq!(player.x16, -28 * 16, "seed={seed}");
    }
}

#[test]
fn invalid_input_is_atomic() {
    let world = scene(1, false);
    let mut entities = EntityWorld::new();
    let first = entities.spawn_player(8 * 16, 24 * 16, &world).unwrap();
    let second = entities.spawn_player(32 * 16, 24 * 16, &world).unwrap();
    let before = entities.state_hash();
    let right = PlayerInput { horizontal: 1, jump: false };
    assert_eq!(
        entities.step_players(&[(first, right), (EntityId(99), right)], &world),
        Err(EntityError::InvalidInput)
    );
    assert_eq!(entities.state_hash(), before);
    assert_eq!(entities.step_players(&[(first, right), (second, right)], &world), Ok(()));
    assert_ne!(entities.state_hash(), before);
    assert_eq!(
        entities.step_players(&[(first, right), (first, right)], &world),
        Err(EntityError::InvalidInput)
    );
}
