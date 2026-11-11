//! 反応の場面テスト (仕様 03 §1)。

use charon_core::coords::CellPos;
use charon_core::testkit::*;

#[test]
fn water_lava_makes_stone_and_steam() {
    for seed in 1..=8 {
        let mut world = world_from_ascii("#####\n#...#\n#wl.#\n#####", seed);
        let stone_before = count(&world, '#');
        assert_sleeps(&mut world, 5_000);
        assert_eq!(count(&world, 'w'), 0, "seed {seed}: {}", to_ascii(&world, 0, 0, 5, 4));
        assert_eq!(count(&world, 'l'), 0, "seed {seed}: {}", to_ascii(&world, 0, 0, 5, 4));
        assert_eq!(count(&world, 't'), 1, "seed {seed}");
        assert_eq!(count(&world, '#'), stone_before + 1, "seed {seed}");
    }
}

#[test]
fn lifetime_decays_into() {
    for seed in 1..=8 {
        let mut world = world_from_ascii("#####\n#k..#\n#####", seed);
        assert_eq!(count(&world, 'k'), 1);
        assert_sleeps(&mut world, 2_000);
        assert_eq!(count(&world, 'k'), 0, "seed {seed}: {}", to_ascii(&world, 0, 0, 5, 3));
    }
}

#[test]
fn wood_burns_to_ash() {
    for seed in 1..=8 {
        let mut world = world_from_ascii("#######\n#.....#\n#WWW..#\n#######", seed);
        let p = CellPos::new(2, 2);
        let mut wood = world.get(p);
        wood.temp = 900;
        world.set(p, wood);
        let mut saw_smoke = false;
        for _ in 0..3_000 {
            world.step();
            saw_smoke |= count(&world, 'k') > 0;
            if world.awake_chunks() == 0 {
                break;
            }
        }
        assert_eq!(world.awake_chunks(), 0, "seed {seed}: {}", to_ascii(&world, 0, 0, 7, 4));
        assert_eq!(count(&world, 'W'), 0, "seed {seed}: {}", to_ascii(&world, 0, 0, 7, 4));
        assert_eq!(count(&world, 'h'), 3, "seed {seed}");
        assert!(saw_smoke, "seed {seed}: 燃焼でできた火が煙になる");
    }
}

#[test]
fn fire_dies_out() {
    for seed in 1..=8 {
        let mut world = world_from_ascii("#######\n#..f..#\n#######", seed);
        assert_sleeps(&mut world, 2_000);
        assert_eq!(count(&world, 'f'), 0, "seed {seed}");
        assert_eq!(count(&world, 'k'), 0, "seed {seed}");
    }
}

#[test]
fn methane_explodes_into_fire() {
    for seed in 1..=8 {
        let mut world = world_from_ascii("#####\n#g..#\n#####", seed);
        let p = CellPos::new(1, 1);
        let mut methane = world.get(p);
        methane.temp = 300;
        world.set(p, methane);
        let mut saw_fire = false;
        for _ in 0..100 {
            world.step();
            saw_fire |= count(&world, 'f') > 0;
            if saw_fire {
                break;
            }
        }
        assert!(saw_fire, "seed {seed}: {}", to_ascii(&world, 0, 0, 5, 3));
        assert_sleeps(&mut world, 2_000);
        assert_eq!(count(&world, 'g'), 0, "seed {seed}");
        assert_eq!(count(&world, 'f'), 0, "seed {seed}");
    }
}

#[test]
fn reaction_is_symmetric() {
    for seed in 1..=8 {
        // 移動先の差が反応の向きに混ざらないよう、二つのセルを石で囲う。
        let mut left = world_from_ascii("#####\n#wl##\n#####", seed);
        let mut right = world_from_ascii("#####\n#lw##\n#####", seed);
        assert_sleeps(&mut left, 5_000);
        assert_sleeps(&mut right, 5_000);
        for ch in ['#', 'w', 'l', 't'] {
            assert_eq!(count(&left, ch), count(&right, ch), "seed {seed}, {ch}");
        }
    }
}
