//! T4.5 実機確認: 固定 tick で落ち続ける世界と最小化からの復帰。
//! `cargo run -p charon-app --example live_world`

use charon_core::testkit::{box_art, fill, material_of, world_from_ascii};
use charon_core::world::World;
use winit::event_loop::EventLoop;

fn make_world(seed: u64) -> World {
    let mut world = world_from_ascii(&box_art(160, 120), seed);
    fill(&mut world, 20, 8, 75, 36, 's');
    fill(&mut world, 85, 38, 138, 65, 'w');
    fill(&mut world, 82, 70, 142, 71, '#');
    world
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let world = make_world(1);
    let brush_material = material_of(&world, 's');
    charon_app::run(EventLoop::new()?, world, brush_material, make_world)?;
    Ok(())
}
