//! デスクトップ用サンドボックス。

mod world;

use winit::event_loop::EventLoop;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let save_root = std::path::PathBuf::from("saves/default");
    let (world, streamer, metadata, physics) = world::open(&save_root)?;
    let selected = world::initial_material(&world);
    charon_app::run_with_streaming(
        EventLoop::new()?,
        world,
        selected,
        world::make_world,
        streamer,
        save_root,
        metadata,
        physics,
    )?;
    Ok(())
}
