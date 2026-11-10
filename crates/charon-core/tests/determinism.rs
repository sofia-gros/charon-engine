//! 決定性 (`docs/TESTING.md` §4)。

use std::path::PathBuf;

use charon_core::rng::hash;
use charon_core::testkit::*;
use charon_core::world::World;

/// 決まった場面を作る。`kinds` の材質を箱の中に乱数で撒く。
fn scene(seed: u64, kinds: &[char]) -> World {
    let mut world = world_from_ascii(&box_art(100, 100), seed);
    for y in 0..100 {
        for x in 1..=100 {
            let h = hash(seed, 0, x, y, 0);
            if h.is_multiple_of(3) {
                place(&mut world, x, y, kinds[((h >> 8) % kinds.len() as u32) as usize]);
            }
        }
    }
    world
}

#[test]
fn deterministic() {
    for seed in 1..=4u64 {
        let kinds = ['s', 'w', 'o', 'm', 'g', 'd'];
        let a = hash_trace(&mut scene(seed, &kinds), 300);
        let b = hash_trace(&mut scene(seed, &kinds), 300);
        assert_eq!(a, b, "seed {seed}");
    }
}

#[test]
fn different_seeds_differ() {
    let kinds = ['s', 'w'];
    // 同じ配置から始めても、seed が違えば乱数が変わり、途中経過が変わる
    let mut a = world_from_ascii(&box_art(40, 40), 1);
    let mut b = world_from_ascii(&box_art(40, 40), 2);
    for world in [&mut a, &mut b] {
        fill(world, 10, 0, 30, 10, kinds[0]);
        fill(world, 10, 20, 30, 30, kinds[1]);
    }
    assert_ne!(hash_trace(&mut a, 50), hash_trace(&mut b, 50));
}

/// 場面を `ticks` 進めた `state_hash()` を `tests/golden/<name>.txt` と比べる。
/// `CHARON_BLESS=1` を付けて実行すると、ファイルを現在の値で書き直す。
fn check_golden(name: &str, mut world: World, ticks: u64) {
    run(&mut world, ticks);
    let actual = format!("{:016x}", world.state_hash());
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.txt"));
    if std::env::var_os("CHARON_BLESS").is_some() {
        let dir = path.parent().expect("基準ファイルには親ディレクトリがある");
        std::fs::create_dir_all(dir).expect("tests/golden を作れません");
        std::fs::write(&path, format!("{actual}\n")).expect("基準ファイルを書けません");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("{} を読めません ({e})。CHARON_BLESS=1 で作成する", path.display())
    });
    assert_eq!(
        actual,
        expected.trim(),
        "場面 {name} の結果が基準と違います。仕様を変えたのでなければ不具合です (docs/TESTING.md §4)"
    );
}

#[test]
fn golden_hashes() {
    check_golden("sand_and_water", scene(1, &['s', 'w']), 400);
    check_golden("all_liquids", scene(2, &['w', 'o', 'm']), 400);
    check_golden("powders_and_gas", scene(3, &['s', 'd', 'h', 'g']), 400);
}
