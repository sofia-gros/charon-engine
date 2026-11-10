//! 粉体の山の形を文字で表示する。摩擦 (`friction`) の違いを目で確かめるための道具。
//!
//! 実行: `cargo run -p charon-core --example piles`

use charon_core::testkit::*;

fn main() {
    // 砂 (s)、土 (d)、灰 (h)
    for ch in ['s', 'd', 'h'] {
        let mut world = world_from_ascii(&box_art(70, 45), 1);
        pour(&mut world, 35, ch, 400);
        assert_sleeps(&mut world, 3000);
        println!("{}\n", to_ascii(&world, 0, 5, 72, 46));
    }
}
