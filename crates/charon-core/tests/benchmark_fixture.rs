//! 仕様 08 §2: 計測用の tick 1 が実際に砂を動かすことを保証する。

use charon_core::coords::CellPos;
use charon_core::testkit::{count, falling_million, material_of};

#[test]
fn benchmark_fixture_updates_cells() {
    let mut world = falling_million();
    let sand = material_of(&world, 's');
    assert_eq!(world.tick(), 1);
    assert_eq!(count(&world, 's'), 1_048_576);
    assert!((1..=1024).all(|x| world.get(CellPos::new(x, 1)).material != sand));

    world.step();
    let moved = (1..=1024).filter(|&x| world.get(CellPos::new(x, 1)).material == sand).count();
    assert!(moved > 0, "tick 1 の更新で砂が空行へ移動していません");
    assert!(world.last_tick_cells() > 0, "tick 1 の更新でセルを走査していません");
    assert_eq!(world.tick(), 2);
    assert_eq!(count(&world, 's'), 1_048_576);
}
