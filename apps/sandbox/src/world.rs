//! デスクトップと Android で共有する初期ワールド。

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

use charon_core::cell::{Cell, MaterialId};
use charon_core::testkit::{box_art, fill, material_of, world_from_ascii};
use charon_core::world::World;
use charon_physics::{BodyError, PhysicsEngine};
#[cfg(not(target_arch = "wasm32"))]
use charon_world::generator::NoiseGenerator;
#[cfg(not(target_arch = "wasm32"))]
use charon_world::save::WorldMeta;
#[cfg(not(target_arch = "wasm32"))]
use charon_world::streaming::Streamer;

pub(crate) fn make_world(seed: u64) -> World {
    let mut world = world_from_ascii(&box_art(160, 120), seed);
    fill(&mut world, 20, 8, 75, 36, 's');
    fill(&mut world, 85, 38, 138, 65, 'w');
    fill(&mut world, 82, 70, 142, 71, '#');
    world
}

pub(crate) fn initial_material(world: &World) -> MaterialId {
    material_of(world, 's')
}

/// 手でつかみ回して、中身を注げる開口部が上の C 字の容器。
pub(crate) fn demo_body(world: &World) -> Result<PhysicsEngine, BodyError> {
    let wood = material_of(world, 'W');
    let (width, height) = (21usize, 15usize);
    let mut cells = vec![Cell::default(); width * height];
    for y in 0..height {
        for x in 0..width {
            if x == 0 || x == width - 1 || y == height - 1 {
                cells[y * width + x].material = wood;
            }
        }
    }
    let mut physics = PhysicsEngine::new();
    physics.create_body(width as u16, height as u16, cells, world.materials(), [112.0, 18.0])?;
    Ok(physics)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn open(
    root: &Path,
) -> Result<(World, Streamer, WorldMeta, PhysicsEngine), Box<dyn std::error::Error>> {
    let initial = make_world(1);
    let materials = Arc::clone(initial.materials());
    let saved = WorldMeta::load(root)?;
    let mut metadata = saved.clone().unwrap_or_else(|| WorldMeta::new(1, &materials));
    for name in charon_world::material_names(&materials) {
        if !metadata.materials.contains(&name) {
            metadata.materials.push(name);
        }
    }
    let mut world =
        if saved.is_some() { World::new(Arc::clone(&materials), metadata.seed) } else { initial };
    world.restore_tick(metadata.tick);
    let generator = Arc::new(NoiseGenerator {
        dirt: material_of(&world, 'd'),
        stone: material_of(&world, '#'),
    });
    let physics = match PhysicsEngine::load_bodies(root, &materials, &metadata.materials)? {
        Some(physics) => physics,
        None => demo_body(&world)?,
    };
    let streamer = Streamer::with_saved_names(
        root,
        materials,
        metadata.seed,
        generator,
        3,
        metadata.materials.clone(),
    )?;
    Ok((world, streamer, metadata, physics))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use charon_core::coords::{CellPos, ChunkPos};

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    struct TestDir(std::path::PathBuf);

    impl TestDir {
        fn new() -> Self {
            loop {
                let path = std::env::temp_dir().join(format!(
                    "charon-sandbox-save-{}-{}",
                    std::process::id(),
                    NEXT_DIR.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(err) => panic!("テストの保存場所を作成できない: {err}"),
                }
            }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("テストの保存場所を削除できない");
        }
    }

    #[test]
    fn sandbox_restarts_with_saved_body() {
        let dir = TestDir::new();
        let (world, streamer, metadata, mut physics) = open(&dir.0).unwrap();
        let handle = physics.bodies[0].handle;
        physics.drag_body(0, [128.0, 64.0]);
        let rigid = &mut physics.world.bodies[handle];
        rigid.set_linvel(rigid.translation(), true);
        let expected_cells = physics.bodies[0].cells.clone();
        physics.save_bodies(&dir.0, world.materials(), &metadata.materials).unwrap();
        streamer.finish().unwrap();
        let (_, streamer, _, restored) = open(&dir.0).unwrap();
        assert_eq!(restored.bodies.len(), 1);
        assert_eq!(restored.bodies[0].cells, expected_cells);
        let loaded = &restored.world.bodies[restored.bodies[0].handle];
        assert_eq!((loaded.translation().x, loaded.translation().y), (8.0, 4.0));
        assert_eq!((loaded.linvel().x, loaded.linvel().y), (8.0, 4.0));
        streamer.finish().unwrap();
    }

    #[test]
    fn sandbox_restarts_with_saved_world() {
        for _seed in 1..=8 {
            let dir = TestDir::new();
            let (mut world, mut streamer, mut metadata, _) = open(&dir.0).unwrap();
            let changed = CellPos::new(10, 10);
            let dirt = material_of(&world, 'd');
            world.set(changed, world.make_cell(dirt, changed));
            for _ in 0..45 {
                streamer.tick(&mut world, &[ChunkPos::new(0, 0)]).unwrap();
                world.restore_tick(world.tick() + 1);
            }
            let original = world.state_hash();
            let saved_tick = world.tick();
            streamer.save_loaded(&world).unwrap();
            streamer.finish().unwrap();
            metadata.tick = saved_tick;
            metadata.save(&dir.0).unwrap();
            let (mut restored, mut streamer, metadata, _) = open(&dir.0).unwrap();
            assert_eq!(metadata.tick, saved_tick);
            assert_eq!(restored.tick(), saved_tick);
            for _ in 0..45 {
                streamer.tick(&mut restored, &[ChunkPos::new(0, 0)]).unwrap();
                restored.restore_tick(restored.tick() + 1);
            }
            assert_eq!(restored.state_hash(), original);
            assert_eq!(restored.get(changed).material, dirt);
            streamer.finish().unwrap();
        }
    }
}
