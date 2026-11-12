#![allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。

use charon_core::cell::Cell;
use charon_core::testkit::{box_art, world_from_ascii};
use charon_core::world::World;
use charon_physics::{CELLS_PER_METER, PhysicsEngine};

/// 16×16 の十字 (`X`) の剛体を、空中 (中心 `(31, 28)`) に 1 つ置く。
fn scene(seed: u64) -> (World, PhysicsEngine) {
    let mut world = world_from_ascii(&box_art(62, 64), seed);
    let ice = world.materials().id("ice").unwrap();
    let mut cells = vec![Cell::default(); 16 * 16];
    for y in 0..16usize {
        for x in 0..16usize {
            if (x as i32 - y as i32).abs() <= 1 || (x as i32 + y as i32 - 15).abs() <= 1 {
                cells[y * 16 + x].material = ice;
            }
        }
    }
    let mut engine = PhysicsEngine::new();
    engine.create_body(16, 16, cells, world.materials(), [31.0, 28.0]).unwrap();
    engine.stamp_all(&mut world);
    (world, engine)
}

fn state(engine: &PhysicsEngine) -> ([f32; 2], f32) {
    let rigid = &engine.world.bodies[engine.bodies[0].handle];
    let t = rigid.translation();
    ([t.x * CELLS_PER_METER, t.y * CELLS_PER_METER], rigid.rotation().angle())
}

/// 角度の差を (-π, π] に収める。
fn wrap(delta: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let d = delta.rem_euclid(tau);
    if d > std::f32::consts::PI { d - tau } else { d }
}

#[test]
fn spinning_body_turns_in_place() {
    for seed in 1..=8 {
        let (mut world, mut engine) = scene(seed);
        assert!(engine.set_spin(0, Some(3.0)));
        assert_eq!(engine.bodies[0].spin, Some(3.0));
        let (start, mut angle) = state(&engine);
        let mut turned = 0.0;
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..120 {
            engine.step(&mut world).unwrap();
            let (position, now) = state(&engine);
            turned += wrap(now - angle);
            angle = now;
            // 位置は動かない (重力でも落ちない)。
            assert!((position[0] - start[0]).abs() < 0.01 && (position[1] - start[1]).abs() < 0.01);
            seen.insert(engine.bodies[0].stamped.clone());
        }
        // 120 tick = 2 秒で、約 3 × 2 = 6 ラジアン回る。
        assert!((turned - 6.0).abs() < 0.3, "seed={seed}: {turned} ラジアン");
        // 回るたびにスタンプの位置が変わる。セルの数はほぼ保たれる。
        assert!(seen.len() > 60, "seed={seed}: {} 通りのスタンプ", seen.len());
        let counts: Vec<usize> = seen.iter().map(Vec::len).collect();
        let (lo, hi) = (counts.iter().min().unwrap(), counts.iter().max().unwrap());
        assert!(*lo as f32 > *hi as f32 * 0.8, "seed={seed}: セル数 {lo}..{hi}");
    }
}

#[test]
fn spin_can_be_stopped_and_rejected() {
    let (mut world, mut engine) = scene(1);
    // 指定が不正なとき・剛体がないときは何も変えない。
    assert!(!engine.set_spin(5, Some(1.0)));
    assert!(!engine.set_spin(0, Some(f32::NAN)));
    assert!(!engine.set_spin(0, Some(f32::INFINITY)));
    assert_eq!(engine.bodies[0].spin, None);

    // 止めると動的な剛体に戻り、重力で落ちる。
    engine.set_spin(0, Some(3.0));
    for _ in 0..10 {
        engine.step(&mut world).unwrap();
    }
    let held = state(&engine).0;
    assert!(engine.set_spin(0, None));
    assert_eq!(engine.bodies[0].spin, None);
    for _ in 0..30 {
        engine.step(&mut world).unwrap();
    }
    assert!(state(&engine).0[1] > held[1] + 2.0, "動的に戻って落ちる");

    // つかんで回しても、回し続ける指定は保たれる。
    let (mut world, mut engine) = scene(2);
    engine.set_spin(0, Some(2.0));
    engine.begin_grab(0);
    engine.rotate_body(0, 0.3);
    let rigid = &engine.world.bodies[engine.bodies[0].handle];
    assert!((rigid.angvel() - 2.0).abs() < 1e-4, "つかんでも角速度を保つ");
    engine.end_grab(0);
    for _ in 0..10 {
        engine.step(&mut world).unwrap();
    }
}

#[test]
fn spinning_is_deterministic() {
    let run = |seed: u64| {
        let (mut world, mut engine) = scene(seed);
        engine.set_spin(0, Some(3.0));
        let mut hashes = Vec::new();
        for _ in 0..90 {
            engine.step(&mut world).unwrap();
            let (p, a) = state(&engine);
            hashes.push((world.state_hash(), p[0].to_bits(), p[1].to_bits(), a.to_bits()));
        }
        hashes
    };
    for seed in 1..=8 {
        assert_eq!(run(seed), run(seed), "seed={seed}");
    }
}
