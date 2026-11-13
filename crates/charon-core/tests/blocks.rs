//! 仕様 17 §2: ブロック (`FLAG_BLOCK`) のセルの規則。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use charon_core::cell::{Cell, FLAG_BLOCK, FLAG_FALLING, MaterialId};
use charon_core::coords::CellPos;
use charon_core::testkit::{
    assert_sleeps, box_art, count, count_block_cells, material_of, place_block, run, to_ascii,
    world_from_ascii,
};
use charon_core::world::World;

/// 高さのある箱 (内側 64×64 の空間の下が床)。ブロック (bx, by) の格子に揃う位置を使う。
fn arena() -> World {
    // 内側は x = 1..=64、床は y = 64。ブロックは 8 の倍数の座標に置く。
    world_from_ascii(&box_art(64, 64), 1)
}

fn remove_cell(world: &mut World, x: i32, y: i32) {
    let pos = CellPos::new(x, y);
    world.set(pos, Cell::default());
}

/// 空中に置いた砂のブロックは、粉でも落ちない。ブロックだけの世界は、そのまま眠る。
#[test]
fn block_cells_do_not_move_even_when_powder() {
    let mut world = arena();
    place_block(&mut world, 2, 2, 's'); // x 16..=23、y 16..=23。下は空いている。
    let before = to_ascii(&world, 16, 14, 23, 26);
    run(&mut world, 200);
    assert_eq!(to_ascii(&world, 16, 14, 23, 26), before, "砂のブロックは落ちない");
    assert_eq!(count_block_cells(&world, 0, 0, 70, 70), 64);
    assert_sleeps(&mut world, 100);
}

/// 砂のブロックは、1 セル欠けると、同じブロックの全体が、最大 14 tick で旗を外して粉になり、崩れて、床に積もる。
#[test]
fn sand_block_crumbles_when_one_cell_is_removed() {
    let mut world = arena();
    place_block(&mut world, 2, 2, 's');
    let sand = count(&world, 's');
    remove_cell(&mut world, 19, 19);
    let mut ticks = 0;
    while count_block_cells(&world, 0, 0, 70, 70) > 0 {
        world.step();
        ticks += 1;
        assert!(ticks <= 14, "14 tick 以内に、すべて旗が外れる ({ticks})");
    }
    assert!(ticks >= 3, "隣へ広がるので、すぐには終わらない ({ticks})");
    run(&mut world, 400);
    assert_eq!(count(&world, 's'), sand - 1, "砂は消えない (欠けた 1 セルのほかは、すべて残る)");
    assert_sleeps(&mut world, 400);
    // 崩れた砂は、床まで落ちている (元の位置には残らない)。
    assert_eq!(world.get(CellPos::new(16, 16)).material, MaterialId::EMPTY);
}

/// 石のブロックは、1 セル欠けても、残りは旗を外すだけで、その場に残る。
#[test]
fn stone_block_stays_solid_after_a_cell_is_removed() {
    let mut world = arena();
    place_block(&mut world, 2, 2, '#');
    remove_cell(&mut world, 19, 19);
    run(&mut world, 60);
    assert_eq!(count_block_cells(&world, 0, 0, 70, 70), 0, "旗はすべて外れる");
    let stone = material_of(&world, '#');
    for (x, y) in [(16, 16), (23, 23), (16, 23), (23, 16)] {
        assert_eq!(world.get(CellPos::new(x, y)).material, stone, "({x}, {y}) は残る");
    }
    assert_sleeps(&mut world, 100);
}

/// 崩れは、同じ 8×8 の中だけで、隣のブロックには連鎖しない。
#[test]
fn crumbling_does_not_cross_block_borders() {
    let mut world = arena();
    place_block(&mut world, 2, 2, 's');
    place_block(&mut world, 3, 2, 's'); // 右隣。x 24..=31。
    place_block(&mut world, 2, 3, 's'); // 下隣。y 24..=31。
    remove_cell(&mut world, 19, 19);
    run(&mut world, 60);
    assert_eq!(count_block_cells(&world, 16, 16, 23, 23), 0, "欠けたブロックは崩れた");
    assert_eq!(count_block_cells(&world, 24, 16, 31, 23), 64, "右隣は無事");
    assert_eq!(count_block_cells(&world, 16, 24, 23, 31), 64, "下隣は無事");
}

/// 材質が変わる処理 (`convert`) は、旗を落とす。
#[test]
fn convert_clears_the_block_flag() {
    let mut world = arena();
    place_block(&mut world, 2, 2, '#');
    let pos = CellPos::new(18, 18);
    assert!(world.get(pos).has(FLAG_BLOCK));
    let sand = material_of(&world, 's');
    assert!(world.convert(pos, sand));
    let cell = world.get(pos);
    assert_eq!(cell.material, sand);
    assert!(!cell.has(FLAG_BLOCK), "材質が変わったら、ブロックではなくなる");
    // 隣のセルは、崩れの規則で旗を外す。
    run(&mut world, 30);
    assert_eq!(count_block_cells(&world, 16, 16, 23, 23), 0);
}

/// ブロックの旗は、`Cell` の 8 ビット目で、ほかの旗と重ならない。
#[test]
fn block_flag_is_its_own_bit() {
    use charon_core::cell::{
        FLAG_BODY, FLAG_BOUNCED, FLAG_BURNING, FLAG_FLOW_RIGHT, FLAG_PARITY, FLAG_SMOOTH,
    };
    let others = [
        FLAG_PARITY,
        FLAG_FALLING,
        FLAG_FLOW_RIGHT,
        FLAG_BURNING,
        FLAG_BODY,
        FLAG_BOUNCED,
        FLAG_SMOOTH,
    ];
    for flag in others {
        assert_eq!(flag & FLAG_BLOCK, 0);
    }
    assert_eq!(FLAG_BLOCK, 0x80);
    let cell = Cell::default().with(FLAG_BLOCK, true);
    assert!(cell.has(FLAG_BLOCK) && !cell.has(FLAG_FALLING));
}

/// 同じ手順なら、同じ結果 (崩れを含む)。
#[test]
fn crumbling_is_deterministic() {
    let run_once = || {
        let mut world = arena();
        place_block(&mut world, 2, 2, 's');
        place_block(&mut world, 3, 2, 's');
        remove_cell(&mut world, 19, 19);
        remove_cell(&mut world, 27, 20);
        let mut trace = Vec::new();
        for _ in 0..120 {
            world.step();
            trace.push(world.state_hash());
        }
        trace
    };
    assert_eq!(run_once(), run_once());
}
