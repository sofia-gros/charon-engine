#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::collections::BTreeSet;
use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::world::World;
use charon_game::projectile::{MAX_PROJECTILES, Payload, ProjectileError, ProjectileWorld};
use charon_game::wand::{CastError, CastOutcome, SpellError, SpellId, SpellTable, Wand, WandError};

const SPELLS: &str = r#"[
    (name: "spark", mana_cost: 5, cast_delay: 6, speed: 128, gravity: 0, life: 60,
     damage: 8, spread: 0, payload: None),
    (name: "bomb", mana_cost: 30, cast_delay: 30, speed: 96, gravity: 2, life: 120,
     damage: 20, spread: 8, payload: Explode(radius: 6)),
    (name: "sand_shot", mana_cost: 10, cast_delay: 10, speed: 96, gravity: 2, life: 90,
     damage: 0, spread: 16, payload: Deposit(material: "sand")),
]"#;

fn materials() -> MaterialTable {
    MaterialTable::builtin().expect("組み込み材質を読み込む")
}

fn world() -> World {
    let mut world = World::new(Arc::new(materials()), 1);
    world.insert_chunk(ChunkPos::new(0, 0));
    world
}

fn table() -> SpellTable {
    SpellTable::parse(SPELLS, &materials(), None).unwrap()
}

/// 1 つの呪文だけの RON。`field` の値を差し替えられる。
fn one_spell(name: &str, field: &str, value: &str) -> String {
    let mut fields = [
        ("mana_cost", "5"),
        ("cast_delay", "6"),
        ("speed", "100"),
        ("gravity", "0"),
        ("life", "60"),
        ("damage", "1"),
        ("spread", "0"),
        ("payload", "None"),
    ];
    for f in &mut fields {
        if f.0 == field {
            f.1 = value;
        }
    }
    let body: Vec<String> = fields.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    format!("[(name: \"{name}\", {})]", body.join(", "))
}

#[test]
fn spell_table_parses_and_resolves() {
    let materials = materials();
    let table = SpellTable::parse(SPELLS, &materials, None).unwrap();
    assert_eq!(table.len(), 3);
    let names: Vec<&str> = table.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["spark", "bomb", "sand_shot"], "記載順");
    assert_eq!(table.id("bomb"), Some(SpellId(1)));
    assert_eq!(table.id("nothing"), None);
    let bomb = table.get(SpellId(1)).unwrap();
    assert_eq!((bomb.mana_cost, bomb.cast_delay, bomb.speed, bomb.gravity), (30, 30, 96, 2));
    assert_eq!(bomb.payload, Payload::Explode { radius: 6 });
    let sand = table.get(SpellId(2)).unwrap();
    assert_eq!(sand.payload, Payload::Deposit { material: materials.id("sand").unwrap() });
    assert!(table.get(SpellId(3)).is_none());

    let namespaced = SpellTable::parse(SPELLS, &materials, Some("demo")).unwrap();
    assert_eq!(namespaced.id("demo:spark"), Some(SpellId(0)));
    assert_eq!(namespaced.id("spark"), None);
}

#[test]
fn spell_table_rejects_bad_data() {
    let materials = materials();
    let parse = |text: &str| SpellTable::parse(text, &materials, None).unwrap_err();
    assert!(matches!(parse("[(name: "), SpellError::Parse(_)));
    assert_eq!(parse("[]"), SpellError::Count(0));
    let many: Vec<String> = (0..257)
        .map(|i| {
            format!(
                "(name: \"s{i}\", mana_cost: 1, cast_delay: 1, speed: 1, gravity: 0, life: 1, \
                 damage: 0, spread: 0, payload: None)"
            )
        })
        .collect();
    assert_eq!(parse(&format!("[{}]", many.join(","))), SpellError::Count(257));

    let two = format!("{}{}", &one_spell("a", "speed", "1")[..], "");
    let dup = two.replacen(']', ", ", 1) + &one_spell("a", "speed", "1")[1..];
    assert_eq!(parse(&dup), SpellError::Duplicate("a".to_owned()));
    // 同じ名前でも、名前空間が違えば別 (ここでは 1 つの表の中なので同じ)。
    assert!(SpellTable::parse(&one_spell("a", "speed", "1"), &materials, Some("m")).is_ok());

    let deposit = |name: &str| format!("Deposit(material: \"{name}\")");
    assert!(matches!(
        parse(&one_spell("a", "payload", &deposit("no_such_material"))),
        SpellError::UnknownMaterial { .. }
    ));
    for (field, bad) in [
        ("mana_cost", "-1"),
        ("mana_cost", "10001"),
        ("cast_delay", "601"),
        ("speed", "0"),
        ("speed", "513"),
        ("gravity", "65"),
        ("gravity", "-65"),
        ("life", "0"),
        ("life", "3601"),
        ("damage", "10001"),
        ("spread", "129"),
        ("payload", "Explode(radius: 0)"),
        ("payload", "Explode(radius: 17)"),
    ] {
        assert!(
            matches!(parse(&one_spell("a", field, bad)), SpellError::Invalid { .. }),
            "{field}={bad}"
        );
    }
    for name in ["", "has space", "日本語", &"x".repeat(65)] {
        assert!(matches!(parse(&one_spell(name, "speed", "1")), SpellError::Invalid { .. }));
    }
    // 範囲の端は通る。
    for (field, ok) in [("speed", "512"), ("life", "3600"), ("spread", "128"), ("gravity", "-64")] {
        assert!(SpellTable::parse(&one_spell("a", field, ok), &materials, None).is_ok());
    }

    let table = table();
    let ids = |n: u16| (0..n).map(|_| SpellId(0)).collect::<Vec<_>>();
    assert_eq!(Wand::new(vec![], 10, 1, 0, &table).unwrap_err(), WandError::SpellCount);
    assert_eq!(Wand::new(ids(17), 10, 1, 0, &table).unwrap_err(), WandError::SpellCount);
    assert_eq!(
        Wand::new(vec![SpellId(3)], 10, 1, 0, &table).unwrap_err(),
        WandError::UnknownSpell(SpellId(3))
    );
    assert_eq!(Wand::new(ids(1), 0, 1, 0, &table).unwrap_err(), WandError::InvalidMana);
    assert_eq!(Wand::new(ids(1), 10_001, 1, 0, &table).unwrap_err(), WandError::InvalidMana);
    assert!(Wand::new(ids(16), 10_000, 0, 0, &table).is_ok());
}

#[test]
fn wand_casts_in_order_and_recharges() {
    let (world, table) = (world(), table());
    let mut projectiles = ProjectileWorld::new();
    let mut wand = Wand::new(vec![SpellId(0), SpellId(1)], 100, 0, 20, &table).unwrap();
    let cast = |wand: &mut Wand, projectiles: &mut ProjectileWorld, tick| {
        wand.tick();
        wand.cast(&table, projectiles, &world, (8 * 16, 8 * 16), (1, 0), tick, 1).unwrap()
    };

    let CastOutcome::Fired(first) = cast(&mut wand, &mut projectiles, 0) else {
        panic!("最初の呪文が撃てない");
    };
    assert_eq!(projectiles.get(first).unwrap().payload, Payload::None);
    assert_eq!((wand.cursor(), wand.cooldown(), wand.mana), (1, 6, 95));
    for tick in 1..6 {
        assert_eq!(cast(&mut wand, &mut projectiles, tick), CastOutcome::Cooling);
    }
    let CastOutcome::Fired(second) = cast(&mut wand, &mut projectiles, 6) else {
        panic!("詠唱間隔のあとに撃てない");
    };
    assert_eq!(projectiles.get(second).unwrap().payload, Payload::Explode { radius: 6 });
    // 最後の呪文のあとは cast_delay に recharge が足され、先頭に戻る。
    assert_eq!((wand.cursor(), wand.cooldown(), wand.mana), (0, 30 + 20, 65));
    assert_eq!(projectiles.len(), 2);
}

#[test]
fn wand_mana_limits_casting() {
    let (world, table) = (world(), table());
    let mut projectiles = ProjectileWorld::new();
    let mut wand = Wand::new(vec![SpellId(1)], 30, 0, 0, &table).unwrap();
    let fire = |wand: &mut Wand, projectiles: &mut ProjectileWorld| {
        wand.cast(&table, projectiles, &world, (0, 0), (0, -1), 0, 1).unwrap()
    };
    assert!(matches!(fire(&mut wand, &mut projectiles), CastOutcome::Fired(_)));
    assert_eq!(wand.mana, 0);
    for _ in 0..30 {
        wand.tick();
    }
    assert_eq!(wand.cooldown(), 0);
    let (hash, shots) = (wand.state_hash(), projectiles.state_hash());
    assert_eq!(fire(&mut wand, &mut projectiles), CastOutcome::NoMana);
    assert_eq!((wand.state_hash(), projectiles.state_hash()), (hash, shots), "何も変わらない");
    wand.mana = 30;
    assert!(matches!(fire(&mut wand, &mut projectiles), CastOutcome::Fired(_)));

    // 回復は mana_max で止まる。
    let mut regen = Wand::new(vec![SpellId(0)], 20, 7, 0, &table).unwrap();
    regen.mana = 3;
    regen.tick();
    assert_eq!(regen.mana, 10);
    for _ in 0..10 {
        regen.tick();
    }
    assert_eq!(regen.mana, 20);
}

#[test]
fn aim_sets_velocity() {
    let world = world();
    let table = SpellTable::parse(&one_spell("fast", "speed", "100"), &materials(), None).unwrap();
    let velocity = |aim: (i32, i32)| {
        let mut projectiles = ProjectileWorld::new();
        let mut wand = Wand::new(vec![SpellId(0)], 100, 0, 0, &table).unwrap();
        let out = wand.cast(&table, &mut projectiles, &world, (0, 0), aim, 0, 1);
        out.map(|o| {
            let CastOutcome::Fired(id) = o else { panic!("撃てない") };
            let p = projectiles.get(id).unwrap();
            (p.vx, p.vy)
        })
    };
    assert_eq!(velocity((3, 4)), Ok((60, 80)));
    assert_eq!(velocity((300, 400)), Ok((60, 80)));
    assert_eq!(velocity((-3, -4)), Ok((-60, -80)));
    assert_eq!(velocity((1, 0)), Ok((100, 0)));
    assert_eq!(velocity((0, -7)), Ok((0, -100)));
    assert_eq!(velocity((0, 0)), Err(CastError::InvalidAim));
    assert_eq!(velocity((2_000_000, 0)), Err(CastError::InvalidAim));
    assert_eq!(velocity((0, -1_000_001)), Err(CastError::InvalidAim));
}

#[test]
fn spread_is_deterministic() {
    let world = world();
    let table = SpellTable::parse(&one_spell("wide", "spread", "16"), &materials(), None).unwrap();
    let shoot = |seed: u64, tick: u64, x: i64| {
        let mut projectiles = ProjectileWorld::new();
        let mut wand = Wand::new(vec![SpellId(0)], 100, 0, 0, &table).unwrap();
        let CastOutcome::Fired(id) =
            wand.cast(&table, &mut projectiles, &world, (x * 16, 0), (1, 0), tick, seed).unwrap()
        else {
            panic!("撃てない");
        };
        let p = projectiles.get(id).unwrap();
        (p.vx, p.vy)
    };
    let mut seen = BTreeSet::new();
    for tick in 0..60 {
        let v = shoot(7, tick, 3);
        assert_eq!(v, shoot(7, tick, 3), "同じ入力で同じ速度");
        assert_eq!(v.0, 100, "散らしは進行方向に垂直");
        assert!(v.1.abs() <= 6, "spread 16 → 100 * 16 / 256 = 6 まで");
        seen.insert(v.1);
    }
    assert!(seen.len() >= 3, "tick が違えば散る: {seen:?}");
    assert_ne!(
        (0..60).map(|t| shoot(7, t, 3)).collect::<Vec<_>>(),
        (0..60).map(|t| shoot(8, t, 3)).collect::<Vec<_>>(),
        "seed が違えば並びが変わる"
    );
}

#[test]
fn failed_cast_changes_nothing() {
    let (world, table) = (world(), table());
    let mut projectiles = ProjectileWorld::new();
    for _ in 0..MAX_PROJECTILES {
        projectiles.spawn(&world, 0, 0, 0, 0, 0, 10, 0, Payload::None).unwrap();
    }
    let mut wand = Wand::new(vec![SpellId(0), SpellId(1)], 50, 0, 0, &table).unwrap();
    let (wand_hash, shots) = (wand.state_hash(), projectiles.state_hash());
    let err = wand.cast(&table, &mut projectiles, &world, (0, 0), (1, 0), 0, 1).unwrap_err();
    assert_eq!(err, CastError::Projectile(ProjectileError::TooMany));
    assert_eq!((wand.state_hash(), projectiles.state_hash()), (wand_hash, shots));
    assert_eq!((wand.mana, wand.cursor(), wand.cooldown()), (50, 0, 0));
}
