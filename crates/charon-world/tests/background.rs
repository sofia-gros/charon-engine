use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use charon_core::cell::{FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_world::background::{BackgroundError, BackgroundWorld};
use charon_world::building::{BuildError, BuildOp, Layer, apply_build};
use charon_world::material_names;

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);
struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "charon-background-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("テスト用ディレクトリを作れません: {err}"),
            }
        }
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("テスト用ディレクトリを削除する");
    }
}

#[test]
fn building_does_not_overwrite() {
    for seed in 1..=8 {
        let mut world = testkit::world_from_ascii("#...\n....", seed);
        let mut background = BackgroundWorld::new();
        let stone = testkit::material_of(&world, '#');
        let wood = testkit::material_of(&world, 'W');
        let at = CellPos::new(0, 0);
        assert_eq!(
            apply_build(
                &mut world,
                &mut background,
                BuildOp { pos: at, layer: Layer::Foreground, material: wood }
            ),
            Err(BuildError::Blocked)
        );
        assert_eq!(world.get(at).material, stone);
        let body = CellPos::new(1, 0);
        world.set(body, world.make_cell(wood, body).with(FLAG_BODY, true));
        assert_eq!(
            apply_build(
                &mut world,
                &mut background,
                BuildOp { pos: body, layer: Layer::Foreground, material: MaterialId::EMPTY }
            ),
            Err(BuildError::Blocked)
        );
        assert!(world.get(body).has(FLAG_BODY));
        let free = CellPos::new(2, 0);
        assert_eq!(
            apply_build(
                &mut world,
                &mut background,
                BuildOp { pos: free, layer: Layer::Foreground, material: wood }
            ),
            Ok(true)
        );
        assert_eq!(world.get(free).material, wood);
        assert_eq!(
            apply_build(
                &mut world,
                &mut background,
                BuildOp { pos: free, layer: Layer::Foreground, material: MaterialId::EMPTY }
            ),
            Ok(true)
        );
        assert_eq!(world.get(free).material, MaterialId::EMPTY);
        assert_eq!(
            apply_build(
                &mut world,
                &mut background,
                BuildOp { pos: free, layer: Layer::Background, material: wood }
            ),
            Ok(true)
        );
        assert_eq!(background.get(free), wood);
        background.take_render_dirty(free.chunk());
        assert_eq!(
            apply_build(
                &mut world,
                &mut background,
                BuildOp { pos: free, layer: Layer::Background, material: wood }
            ),
            Ok(false)
        );
        assert_eq!(background.chunk(free.chunk()).unwrap().render_dirty, None);
        assert_eq!(
            apply_build(
                &mut world,
                &mut background,
                BuildOp { pos: free, layer: Layer::Background, material: MaterialId(999) }
            ),
            Err(BuildError::InvalidMaterial(MaterialId(999)))
        );
    }
}

#[test]
fn background_does_not_affect_simulation() {
    for seed in 1..=8 {
        let mut a = testkit::world_from_ascii("#....#\n#..s.#\n#....#\n######", seed);
        let mut b = testkit::world_from_ascii("#....#\n#..s.#\n#....#\n######", seed);
        let mut background = BackgroundWorld::new();
        background.set(CellPos::new(3, 2), testkit::material_of(&a, 'W'));
        for _ in 0..100 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash(), "seed={seed}");
        }
        assert_ne!(background.get(CellPos::new(3, 2)), MaterialId::EMPTY);
    }
}

#[test]
fn background_roundtrip() {
    let dir = TestDir::new();
    let materials = MaterialTable::builtin().unwrap();
    let names = material_names(&materials);
    let mut background = BackgroundWorld::new();
    let wood = materials.id("wood").unwrap();
    let stone = materials.id("stone").unwrap();
    background.set(CellPos::new(-1, 64), wood);
    background.set(CellPos::new(65, -1), stone);
    background.save(&dir.0, &materials, &names).unwrap();
    let restored = BackgroundWorld::load(&dir.0, &materials, &names).unwrap();
    assert_eq!(restored.get(CellPos::new(-1, 64)), wood);
    assert_eq!(restored.get(CellPos::new(65, -1)), stone);
    assert!(restored.chunk(CellPos::new(-1, 64).chunk()).unwrap().render_dirty.is_some());
    let reordered = MaterialTable::from_ron(
        r#"[
        (name: "wood", phase: Solid, density: 600, color: (3, 3, 3, 255)),
        (name: "stone", phase: Solid, density: 2600, color: (1, 1, 1, 255)),
    ]"#,
    )
    .unwrap();
    let renamed = BackgroundWorld::load(&dir.0, &reordered, &names).unwrap();
    assert_eq!(renamed.get(CellPos::new(-1, 64)), reordered.id("wood").unwrap());
    assert_eq!(renamed.get(CellPos::new(65, -1)), reordered.id("stone").unwrap());
    background.save(&dir.0, &materials, &names).unwrap(); // 同じファイルを更新できる。
    let mut bytes = fs::read(dir.0.join("background.bin")).unwrap();
    bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
    fs::write(dir.0.join("background.bin"), bytes).unwrap();
    assert!(matches!(
        BackgroundWorld::load(&dir.0, &materials, &names),
        Err(BackgroundError::NewerVersion(2))
    ));
}
