#![allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。

use charon_core::cell::{Cell, FLAG_BODY, FLAG_SMOOTH};
use charon_core::coords::CellPos;
use charon_core::testkit::{box_art, world_from_ascii};
use charon_core::world::World;
use charon_physics::{BodyRender, CELLS_PER_METER, PhysicsEngine};

/// 16×16 の十字 (`X`、線の太さ 5 セル) の剛体を、空中 (中心 `(31, 28)`) に 1 つ置く。
fn scene(seed: u64) -> (World, PhysicsEngine) {
    let mut world = world_from_ascii(&box_art(62, 64), seed);
    let ice = world.materials().id("ice").unwrap();
    let mut cells = vec![Cell::default(); 16 * 16];
    for y in 0..16usize {
        for x in 0..16usize {
            if (x as i32 - y as i32).abs() <= 2 || (x as i32 + y as i32 - 15).abs() <= 2 {
                cells[y * 16 + x].material = ice;
            }
        }
    }
    let mut engine = PhysicsEngine::new();
    engine.create_body(16, 16, cells, world.materials(), [31.0, 28.0]).unwrap();
    engine.set_spin(0, Some(0.0)); // 運動学的にして、空中に留める。
    engine.stamp_all(&mut world);
    (world, engine)
}

fn stamped_flags(world: &World, engine: &PhysicsEngine) -> Vec<(bool, bool)> {
    engine.bodies[0]
        .stamped
        .iter()
        .map(|&pos: &CellPos| {
            let cell = world.get(pos);
            (cell.has(FLAG_BODY), cell.has(FLAG_SMOOTH))
        })
        .collect()
}

#[test]
fn smooth_body_stamps_flagged_cells() {
    for seed in 1..=8 {
        let (mut world, mut engine) = scene(seed);
        assert_eq!(engine.bodies[0].render, BodyRender::Dots, "既定はドット");
        let dots = stamped_flags(&world, &engine);
        assert!(dots.len() > 80, "seed={seed}");
        assert!(dots.iter().all(|&(body, smooth)| body && !smooth), "ドットは FLAG_BODY だけ");

        // 滑らかにすると、次のスタンプから FLAG_SMOOTH が付く。スタンプしたセルの数は同じ。
        assert!(engine.set_render(0, BodyRender::Smooth));
        engine.step(&mut world).unwrap();
        let smooth = stamped_flags(&world, &engine);
        assert_eq!(smooth.len(), dots.len(), "seed={seed}: 描き方で数は変わらない");
        assert!(smooth.iter().all(|&(body, smooth)| body && smooth), "滑らかは両方のフラグ");

        // ドットに戻すと、FLAG_SMOOTH は消える。
        engine.set_render(0, BodyRender::Dots);
        engine.step(&mut world).unwrap();
        assert!(stamped_flags(&world, &engine).iter().all(|&(body, smooth)| body && !smooth));
        // 取り除いたあとに、剛体のセルは残らない。
        engine.unstamp_all(&mut world);
        let leftover = (0..64)
            .flat_map(|y| (0..64).map(move |x| CellPos::new(x, y)))
            .filter(|&pos| world.get(pos).has(FLAG_SMOOTH))
            .count();
        assert_eq!(leftover, 0);
    }
    assert!(!PhysicsEngine::new().set_render(0, BodyRender::Smooth), "剛体がなければ false");
}

#[test]
fn smooth_flag_does_not_change_the_simulation() {
    // 滑らかでもドットでも、物理と世界の状態 (FLAG_SMOOTH を除く) は同じ。
    let run = |render: BodyRender| {
        let (mut world, mut engine) = scene(3);
        engine.set_render(0, render);
        engine.set_spin(0, Some(2.0));
        let mut trace = Vec::new();
        for _ in 0..90 {
            engine.step(&mut world).unwrap();
            let pose = engine.body_pose(0).unwrap();
            trace.push((
                engine.bodies[0].stamped.clone(),
                pose.center[0].to_bits(),
                pose.center[1].to_bits(),
                pose.angle.to_bits(),
            ));
        }
        trace
    };
    assert_eq!(run(BodyRender::Dots), run(BodyRender::Smooth));
}

#[test]
fn body_pose_matches_the_rigid_body() {
    let (mut world, mut engine) = scene(1);
    engine.set_spin(0, Some(3.0));
    for _ in 0..20 {
        engine.step(&mut world).unwrap();
    }
    let pose = engine.body_pose(0).unwrap();
    let rigid = &engine.world.bodies[engine.bodies[0].handle];
    assert_eq!(pose.center[0], rigid.translation().x * CELLS_PER_METER);
    assert_eq!(pose.center[1], rigid.translation().y * CELLS_PER_METER);
    assert_eq!(pose.angle, rigid.rotation().angle());
    assert!((pose.center[0] - 31.0).abs() < 0.01 && (pose.center[1] - 28.0).abs() < 0.01);
    assert!(pose.angle.abs() > 0.1, "回っている");
    assert!(engine.body_pose(1).is_none());
    assert!(PhysicsEngine::new().body_pose(0).is_none());
}

#[test]
fn broken_smooth_pieces_stay_smooth() {
    for seed in 1..=4 {
        let (mut world, mut engine) = scene(seed);
        engine.set_render(0, BodyRender::Smooth);
        engine.set_spin(0, None);
        // 十字の中心 (腕が交わる 4×4) を壊して、4 本の腕に分ける。
        for y in 5..=10 {
            for x in 5..=10 {
                engine.damage(0, x, y);
            }
        }
        engine.split_damaged(&mut world).unwrap();
        assert_eq!(engine.bodies.len(), 4, "seed={seed}: 4 つの破片に分かれる");
        assert!(
            engine.bodies.iter().all(|body| body.render == BodyRender::Smooth),
            "seed={seed}: 破片は元の描き方を引き継ぐ"
        );
        // 破片をスタンプしても、滑らかなフラグが付く。
        engine.stamp_all(&mut world);
        let smooth_cells: usize = engine
            .bodies
            .iter()
            .map(|body| body.stamped.iter().filter(|&&pos| world.get(pos).has(FLAG_SMOOTH)).count())
            .sum();
        let stamped: usize = engine.bodies.iter().map(|body| body.stamped.len()).sum();
        assert!(stamped > 40 && smooth_cells == stamped, "seed={seed}");
    }
}
