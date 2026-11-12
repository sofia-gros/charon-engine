use std::sync::Arc;

use charon_core::cell::FLAG_BODY;
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::{
    EntityError, EntityId, EntityWorld, PLAYER_HEIGHT, PLAYER_WIDTH, PlayerInput, step_player,
};

/// 高さ 40 の床の上に立つプレイヤーの y16。
const STAND: i64 = (40 - PLAYER_HEIGHT as i64) * 16;
const W: i64 = PLAYER_WIDTH as i64;

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
        assert_eq!(player.y16, STAND, "seed={seed}");
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
        assert_eq!(entities.get(id).unwrap().x16, (18 - W) * 16, "seed={seed}");
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
        let id = entities.spawn_player(-32 * 16, STAND, &world).unwrap();
        entities.step_players(&[], &world).unwrap();
        assert!(entities.get(id).unwrap().grounded);
        for _ in 0..10 {
            entities
                .step_players(&[(id, PlayerInput { horizontal: 1, jump: false })], &world)
                .unwrap();
        }
        assert_eq!(entities.get(id).unwrap().x16, (-20 - W) * 16, "seed={seed}");
        entities.step_players(&[(id, PlayerInput { horizontal: 0, jump: true })], &world).unwrap();
        assert!(entities.get(id).unwrap().y16 < STAND, "seed={seed}");
        for _ in 0..100 {
            entities.step_players(&[], &world).unwrap();
        }
        assert_eq!(entities.get(id).unwrap().y16, STAND, "seed={seed}");
        assert!(entities.get(id).unwrap().grounded);
        // 負座標のセル境界を 1/16 セルだけ横切ったときも衝突する。
        let mut player = entities.get(id).unwrap().clone();
        player.x16 = (-20 - W) * 16 - 1;
        step_player(&mut player, PlayerInput { horizontal: 1, jump: false }, &world);
        assert_eq!(player.x16, (-20 - W) * 16, "seed={seed}");
    }
}

/// 当たり判定は箱に内接する楕円: 箱の角のセルは、固体があっても当たらない。中央のセルは当たる。
#[test]
fn player_collision_is_elliptical() {
    for seed in 1..=8 {
        let (x, y) = (20_i32, 20_i32);
        let (x16, y16) = (i64::from(x) * 16, i64::from(y) * 16);
        // 箱の左上の角のセル。
        let mut world = scene(seed, false);
        testkit::place(&mut world, x, y, '#');
        let mut entities = EntityWorld::new();
        assert!(entities.spawn_player(x16, y16, &world).is_ok(), "seed={seed}: 角は空く");
        // 箱の中心のセル。
        let mut world = scene(seed, false);
        testkit::place(&mut world, x + W as i32 / 2, y + i32::from(PLAYER_HEIGHT) / 2, '#');
        assert_eq!(
            EntityWorld::new().spawn_player(x16, y16, &world),
            Err(EntityError::Occupied),
            "seed={seed}: 中心は当たる"
        );
    }
}

/// 砂に埋まらない: 楕円に重なった粉体は近くの空きへ押し出され、セルの数は変わらない。
#[test]
fn player_pushes_sand_out() {
    for seed in 1..=8 {
        let mut world = scene(seed, false);
        let mut entities = EntityWorld::new();
        let id = entities.spawn_player(8 * 16, STAND, &world).unwrap();
        let top = (STAND / 16) as i32;
        let centre = 8 + W as i32 / 2;
        for dy in 1..i32::from(PLAYER_HEIGHT) - 1 {
            testkit::place(&mut world, centre, top + dy, 's');
        }
        let before = testkit::count(&world, 's');
        assert!(before >= 6);
        let moved = entities.push_out_powder(&mut world);
        assert_eq!(moved, before, "seed={seed}: 重なった砂は全部動く");
        assert_eq!(testkit::count(&world, 's'), before, "seed={seed}: セルは増減しない");
        for dy in 1..i32::from(PLAYER_HEIGHT) - 1 {
            assert_eq!(
                world.get(CellPos::new(centre, top + dy)).material,
                charon_core::cell::MaterialId::EMPTY,
                "seed={seed}: 楕円の中は空く"
            );
        }
        // 床 (固体) には触れず、押し出したあとは何も動かさない。
        assert_eq!(entities.push_out_powder(&mut world), 0);
        assert_eq!(entities.get(id).unwrap().y16, STAND);
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
