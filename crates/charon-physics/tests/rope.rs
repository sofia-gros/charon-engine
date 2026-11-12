use charon_physics::rope::{Rope, RopeEvent, RopeParams, RopePhase, tether_velocity};

fn params(gravity: f32) -> RopeParams {
    RopeParams {
        segments: 12,
        max_length: 100.0,
        min_length: 2.0,
        gravity,
        iterations: 8,
        reel_speed: 0.5,
    }
}

/// 撃って、地形に着くまで進める。着いたら `Some(tick)`。
fn fly(
    rope: &mut Rope,
    hand: [f32; 2],
    p: &RopeParams,
    solid: &mut dyn FnMut([f32; 2]) -> bool,
) -> Option<usize> {
    for tick in 1..=200 {
        match rope.step(hand, false, p, solid) {
            RopeEvent::Anchored => return Some(tick),
            RopeEvent::Lost => return None,
            RopeEvent::None => {}
        }
    }
    None
}

#[test]
fn rope_sags_under_gravity() {
    let p = params(0.05);
    // x = 20 以降が壁。
    let mut solid = |q: [f32; 2]| q[0] >= 20.0;
    let mut rope = Rope::launch([0.0, 10.0], [1.0, 0.0], 4.0, &p).unwrap();
    fly(&mut rope, [0.0, 10.0], &p, &mut solid).expect("壁に着く");
    assert_eq!(rope.phase(), RopePhase::Anchored);
    let anchor = rope.anchor().unwrap();
    // 手を近づけてたるませる。ワイヤーは自分の重さで垂れる。
    let hand = [8.0, 10.0];
    for _ in 0..200 {
        rope.step(hand, false, &p, &mut solid);
    }
    let points = rope.points();
    let line_y = |x: f32| 10.0 + (anchor[1] - 10.0) * (x - hand[0]) / (anchor[0] - hand[0]);
    let sag = points.iter().map(|q| q[1] - line_y(q[0])).fold(f32::MIN, f32::max);
    assert!(sag > 1.0, "両端を結ぶ線より下に垂れる: {sag}");
    assert!(rope.polyline_length() <= rope.rest_length() * 1.05, "伸びない");
    assert!(points.iter().all(|q| q.iter().all(|v| v.is_finite())));
}

#[test]
fn rope_flight_anchors_on_solid() {
    let p = params(0.0);
    for speed in [1.0, 5.0, 20.0, 40.0] {
        let mut solid = |q: [f32; 2]| q[0] >= 30.0;
        let mut rope = Rope::launch([0.0, 0.0], [1.0, 0.0], speed, &p).unwrap();
        let ticks = fly(&mut rope, [0.0, 0.0], &p, &mut solid).expect("着く");
        let anchor = rope.anchor().unwrap();
        assert!(
            anchor[0] < 30.0 && anchor[0] > 30.0 - speed.max(0.5) - 0.01,
            "速さ {speed}: {anchor:?}"
        );
        assert!(ticks as f32 <= 30.0 / speed + 2.0, "速さ {speed}: {ticks} tick");
        assert!(!solid(anchor), "すり抜けず、地形の手前で止まる");
    }
    // 向きが 0 や有限でなければ撃てない。
    assert!(Rope::launch([0.0, 0.0], [0.0, 0.0], 1.0, &p).is_none());
    assert!(Rope::launch([0.0, 0.0], [f32::NAN, 1.0], 1.0, &p).is_none());
    assert!(Rope::launch([0.0, 0.0], [1.0, 0.0], 0.0, &p).is_none());
    // 届かなければ最大の長さで外れる。
    let mut none = |_: [f32; 2]| false;
    let mut far = Rope::launch([0.0, 0.0], [1.0, 0.0], 10.0, &p).unwrap();
    let mut lost = false;
    for _ in 0..50 {
        if far.step([0.0, 0.0], false, &p, &mut none) == RopeEvent::Lost {
            lost = true;
            break;
        }
    }
    assert!(lost);
}

#[test]
fn rope_reeling_shortens_and_stays_taut() {
    let mut p = params(0.0);
    p.reel_speed = 0.1;
    let mut solid = |q: [f32; 2]| q[1] <= -20.0;
    let mut hand = [0.0f32, 0.0];
    let mut rope = Rope::launch(hand, [0.0, -1.0], 5.0, &p).unwrap();
    fly(&mut rope, hand, &p, &mut solid).expect("着く");
    let start_rest = rope.rest_length();
    assert!((start_rest - 20.0).abs() < 5.5, "{start_rest}");
    // 質点 (プレイヤー) をワイヤーで引き寄せる。重力もかかる。
    let mut velocity = [0.0f32, 0.0];
    for _ in 0..400 {
        velocity[1] += 0.25;
        velocity = tether_velocity(&rope, hand, velocity, true, 0.3, 3.0);
        hand = [hand[0] + velocity[0], hand[1] + velocity[1]];
        rope.step(hand, true, &p, &mut solid);
        assert!(
            rope.polyline_length() <= rope.rest_length() + 2.0,
            "張って伸びない: {} / {}",
            rope.polyline_length(),
            rope.rest_length()
        );
    }
    assert_eq!(rope.rest_length(), p.min_length, "巻き取りで下限まで縮む");
    let anchor = rope.anchor().unwrap();
    let distance = (anchor[1] - hand[1]).hypot(anchor[0] - hand[0]);
    assert!(distance <= p.min_length + 2.0, "アンカーの近くまで引かれる: {distance}");
}

#[test]
fn rope_stays_out_of_solids() {
    let p = params(0.375);
    // 床 (y >= 30) と、途中の柱 (x 14..16、y >= 18)。
    let solid = |q: [f32; 2]| q[1] >= 30.0 || (q[0] >= 14.0 && q[0] < 16.0 && q[1] >= 18.0);
    let mut solid_fn = solid;
    let mut hand = [2.0f32, 12.0];
    let mut rope = Rope::launch(hand, [1.0, 0.6], 3.0, &p).unwrap();
    for tick in 0..300 {
        // 手を柱の向こうまで動かす。ワイヤーは柱の角にかかる。
        hand[0] = 2.0 + (tick as f32 * 0.1).min(24.0);
        rope.step(hand, tick > 40, &p, &mut solid_fn);
        for (i, q) in rope.points().iter().enumerate().skip(1) {
            assert!(!solid(*q), "tick {tick}: 点 {i} が地形の中 {q:?}");
        }
    }
}

#[test]
fn rope_is_deterministic() {
    let p = params(0.375);
    let run = |seed: u32| {
        let wall = 10.0 + seed as f32 * 3.0;
        let mut solid = move |q: [f32; 2]| q[0] >= wall || q[1] >= 40.0;
        let mut rope =
            Rope::launch([1.0, 5.0], [1.0, 0.2 * seed as f32], 2.0 + seed as f32, &p).unwrap();
        let mut hashes = Vec::new();
        for tick in 0..120 {
            let hand = [1.0 + (tick as f32 * 0.05).sin(), 5.0];
            rope.step(hand, tick > 30, &p, &mut solid);
            hashes.push(rope.state_hash());
        }
        hashes
    };
    for seed in 1..=8 {
        assert_eq!(run(seed), run(seed), "seed {seed}");
    }
    assert_ne!(run(1), run(2));
}

#[test]
fn tether_removes_outward_velocity() {
    let p = params(0.0);
    let hand = [0.0f32, 0.0];
    let mut solid = |q: [f32; 2]| q[1] <= -10.0;
    let mut rope = Rope::launch(hand, [0.0, -1.0], 4.0, &p).unwrap();
    fly(&mut rope, hand, &p, &mut solid).expect("着く");
    // ワイヤーはアンカーに向かって真上に張っている。
    // 外向き (下向き) の速度は取り除かれる。
    let v = tether_velocity(&rope, hand, [0.0, 1.0], false, 0.5, 100.0);
    assert!(v[1].abs() < 0.2, "{v:?}");
    // 内向き (上向き) の速度は変わらない。
    let v = tether_velocity(&rope, hand, [0.0, -1.0], false, 0.5, 100.0);
    assert!((v[1] + 1.0).abs() < 0.2, "{v:?}");
    // 横向きの速度 (振り子の接線) は変わらない。
    let v = tether_velocity(&rope, hand, [2.0, 0.0], false, 0.5, 100.0);
    assert!((v[0] - 2.0).abs() < 0.15 && v[1].abs() < 0.3, "{v:?}");
    // 巻き取りで内向きに加速し、上限を守る。
    let v = tether_velocity(&rope, hand, [0.0, 0.0], true, 0.5, 100.0);
    assert!((v[1] + 0.5).abs() < 0.2, "{v:?}");
    let v = tether_velocity(&rope, hand, [0.0, -3.0], true, 0.5, 2.0);
    assert!((v[0].hypot(v[1]) - 2.0).abs() < 1e-3, "{v:?}");
    // たるんでいる (手が近い) ときは変えない。
    rope.step([0.0, -6.0], false, &p, &mut solid);
    let free = [3.0, 4.0];
    assert_eq!(tether_velocity(&rope, [0.0, -6.0], free, true, 0.5, 100.0), free);
    // 飛んでいる間は変えない。
    let flying = Rope::launch(hand, [1.0, 0.0], 1.0, &p).unwrap();
    assert_eq!(tether_velocity(&flying, hand, free, true, 0.5, 100.0), free);
}
