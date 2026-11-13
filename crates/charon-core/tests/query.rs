//! 問い合わせ API (仕様 01 §7)。

use charon_core::cell::{Cell, MaterialId};
use charon_core::coords::CellPos;
use charon_core::query::Stop;
use charon_core::testkit::{material_of, world_from_ascii};

fn scene() -> charon_core::world::World {
    world_from_ascii(
        "
        ..........
        ..........
        ....#.....
        ..........
        ..www.....
        ##########
        ",
        1,
    )
}

#[test]
fn raycast_stops_at_the_first_solid() {
    let world = scene();
    let hit = world.raycast(CellPos::new(0, 2), CellPos::new(9, 2), Stop::Solid).expect("hit");
    assert_eq!(hit.pos, CellPos::new(4, 2));
    assert_eq!(hit.before, CellPos::new(3, 2));
    assert_eq!(hit.steps, 4);
    assert_eq!(hit.cell.material, material_of(&world, '#'));
}

#[test]
fn raycast_passes_liquid_unless_asked_not_to() {
    let world = scene();
    assert!(world.raycast(CellPos::new(0, 4), CellPos::new(9, 4), Stop::Solid).is_none());
    let hit = world.raycast(CellPos::new(0, 4), CellPos::new(9, 4), Stop::NotEmpty).expect("hit");
    assert_eq!(hit.pos, CellPos::new(2, 4));
}

#[test]
fn raycast_works_in_every_direction_and_on_diagonals() {
    let world = scene();
    // 右下へ斜め: 床の石に当たる。
    let hit = world.raycast(CellPos::new(0, 0), CellPos::new(5, 5), Stop::Solid).expect("hit");
    assert_eq!(hit.pos.y, 5);
    // 逆向きでも同じ石 (4,2) に当たる。
    let back = world.raycast(CellPos::new(9, 2), CellPos::new(0, 2), Stop::Solid).expect("hit");
    assert_eq!(back.pos, CellPos::new(4, 2));
    // 始点が当たりなら steps は 0。
    let start = world.raycast(CellPos::new(4, 2), CellPos::new(9, 2), Stop::Solid).expect("hit");
    assert_eq!((start.steps, start.before), (0, CellPos::new(4, 2)));
}

#[test]
fn line_of_sight_and_the_outside_of_the_world() {
    let world = scene();
    assert!(world.line_of_sight(CellPos::new(0, 0), CellPos::new(9, 0), Stop::Solid));
    assert!(!world.line_of_sight(CellPos::new(0, 2), CellPos::new(9, 2), Stop::Solid));
    // 読み込まれていないところは境界として遮る。
    let hit = world.raycast(CellPos::new(0, 0), CellPos::new(0, -5000), Stop::Solid).expect("hit");
    assert_eq!(hit.cell, Cell::BOUNDARY);
    assert_eq!(hit.cell.material, MaterialId::BOUNDARY);
}

#[test]
fn circle_search_is_ordered_and_counts() {
    let world = scene();
    let water = material_of(&world, 'w');
    assert_eq!(world.count_in_circle(CellPos::new(3, 4), 1, water), 3);
    let cells = world.cells_in_circle(CellPos::new(3, 4), 2, |c| c.material == water);
    assert_eq!(cells, vec![CellPos::new(2, 4), CellPos::new(3, 4), CellPos::new(4, 4)]);
    assert!(world.cells_in_circle(CellPos::new(3, 4), -3, |_| true).len() == 1);
}
