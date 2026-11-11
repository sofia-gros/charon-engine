#![allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。

use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

use charon_core::cell::{Cell, FLAG_BODY, MaterialId};
use charon_core::testkit::{box_art, material_of, world_from_ascii};
use charon_physics::{BodySaveError, CELLS_PER_METER, PhysicsEngine};
use rapier2d::prelude::{Rotation, Vector};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct TestDir(std::path::PathBuf);
impl TestDir {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "charon-bodies-test-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("テスト用の保存先を作れません: {error}"),
            }
        }
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn bodies_roundtrip() {
    for seed in 1..=8 {
        let world = world_from_ascii(&box_art(64, 64), seed);
        let materials = world.materials();
        let names: Vec<_> = (0..materials.len())
            .map(|index| materials.get(MaterialId(index as u16)).name.clone())
            .collect();
        let wood = material_of(&world, 'W');
        let stone = material_of(&world, '#');
        let cells = vec![
            Cell { material: wood, shade: 13, temp: 45, ..Cell::default() },
            Cell::default(),
            Cell { material: stone, life: 7, ..Cell::default() },
            Cell { material: wood, flags: FLAG_BODY, ..Cell::default() },
        ];
        let mut engine = PhysicsEngine::new();
        let index = engine.create_body(2, 2, cells, materials, [18.0, 22.0]).unwrap();
        let rigid = &mut engine.world.bodies[engine.bodies[index].handle];
        rigid.set_rotation(Rotation::new(0.4), true);
        rigid.set_translation(Vector::new(31.0 / CELLS_PER_METER, 42.0 / CELLS_PER_METER), true);
        rigid.set_linvel(Vector::new(1.5, -2.25), true);
        rigid.set_angvel(0.75, true);
        let dir = TestDir::new();
        assert!(PhysicsEngine::load_bodies(&dir.0, materials, &names).unwrap().is_none());
        engine.save_bodies(&dir.0, materials, &names).unwrap();
        engine.save_bodies(&dir.0, materials, &names).unwrap(); // 既存ファイルの置換も確認。
        let restored = PhysicsEngine::load_bodies(&dir.0, materials, &names).unwrap().unwrap();
        assert_eq!(restored.bodies.len(), 1);
        assert_eq!((restored.bodies[0].width, restored.bodies[0].height), (2, 2));
        let mut expected = engine.bodies[0].cells.clone();
        for cell in &mut expected {
            cell.flags &= !FLAG_BODY;
        }
        assert_eq!(restored.bodies[0].cells, expected);
        let original = &engine.world.bodies[engine.bodies[0].handle];
        let loaded = &restored.world.bodies[restored.bodies[0].handle];
        assert_eq!(loaded.translation(), original.translation());
        assert_eq!(loaded.rotation(), original.rotation());
        assert_eq!(loaded.linvel(), original.linvel());
        assert_eq!(loaded.angvel(), original.angvel());
        assert!(restored.bodies[0].stamped.is_empty());
    }
}

#[test]
fn invalid_body_file_is_rejected() {
    let world = world_from_ascii(&box_art(16, 16), 1);
    let dir = TestDir::new();
    fs::write(dir.0.join("bodies.bin"), [b'C', b'H', b'R', b'B', 2, 0, 0, 0, 0, 0]).unwrap();
    assert!(matches!(
        PhysicsEngine::load_bodies(&dir.0, world.materials(), &[]),
        Err(BodySaveError::NewerVersion(2))
    ));
}
