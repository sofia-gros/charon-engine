#![allow(clippy::unwrap_used)] // 不正なテスト用ファイル操作は直ちに失敗させる。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use charon_core::cell::MaterialId;
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialError;
use charon_core::reaction::ReactionError;
use charon_core::testkit::{assert_sleeps, fill};
use charon_game::mods::{ModDirectory, ModError, ModPack};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "charon-mod-tests-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("テスト用ディレクトリを作れません: {error}"),
            }
        }
    }
    fn add(
        &self,
        dir: &str,
        id: &str,
        materials: &str,
        reactions: &str,
        script: Option<&str>,
    ) -> ModDirectory {
        let path = self.0.join(dir);
        fs::create_dir(&path).unwrap();
        fs::write(path.join("manifest.ron"), format!("(id: \"{id}\", version: 1)")).unwrap();
        fs::write(path.join("materials.ron"), materials).unwrap();
        fs::write(path.join("reactions.ron"), reactions).unwrap();
        if let Some(script) = script {
            fs::write(path.join("script.ron"), script).unwrap();
        }
        ModDirectory { path, script: script.is_some() }
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn fixed(name: &str, extras: &str) -> String {
    format!("[(name: \"{name}\", phase: Solid, density: 100, color: (8, 9, 10, 255){extras})]")
}

fn name(pack: &ModPack, index: usize) -> &str {
    pack.materials.get(MaterialId(index as u16)).name.as_str()
}

#[test]
fn mods_load_in_sorted_order() {
    let dir = TestDir::new();
    let z = dir.add(
        "z-folder",
        "a_mod",
        &fixed("last", ", decay_into: \"b_mod:first\""),
        r#"[(a: "last", b: "b_mod:first", prob: 200, out_a: "empty", out_b: "stone")]"#,
        None,
    );
    let a =
        dir.add("a-folder", "b_mod", &fixed("first", ", decay_into: \"a_mod:last\""), "[]", None);
    let pack = ModPack::load(&[z.clone(), a.clone()]).unwrap();
    let reversed = ModPack::load(&[a, z]).unwrap();
    assert_eq!(pack.manifests, reversed.manifests);
    let names: Vec<_> = (0..pack.materials.len()).map(|i| name(&pack, i)).collect();
    assert_eq!(
        names,
        (0..reversed.materials.len()).map(|i| name(&reversed, i)).collect::<Vec<_>>()
    );
    let first = pack.materials.id("b_mod:first").unwrap();
    let last = pack.materials.id("a_mod:last").unwrap();
    assert!(first < last);
    assert_eq!(pack.materials.get(first).decay_into, last);
    assert_eq!(pack.materials.get(last).decay_into, first);
    assert_eq!(pack.reactions.get(last, first).unwrap().out_b, pack.materials.id("stone").unwrap());
    assert_eq!(pack.reactions.get(first, last).unwrap().out_a, pack.materials.id("stone").unwrap());
}

#[test]
fn duplicate_mod_material_is_rejected() {
    let dir = TestDir::new();
    let first = dir.add("a", "same", &fixed("grain", ""), "[]", None);
    let second = dir.add("b", "same", &fixed("grain", ""), "[]", None);
    assert!(matches!(ModPack::load(&[first.clone(), second]), Err(ModError::DuplicateId(_))));
    fs::write(
        first.path.join("materials.ron"),
        r#"[
        (name: "grain", phase: Solid, color: (8, 9, 10, 255)),
        (name: "grain", phase: Solid, color: (8, 9, 10, 255))
    ]"#,
    )
    .unwrap();
    assert!(matches!(
        ModPack::load(std::slice::from_ref(&first)),
        Err(ModError::Materials(MaterialError::DuplicateName(_)))
    ));
    fs::write(first.path.join("materials.ron"), fixed("grain", ", decay_into: \"unknown:thing\""))
        .unwrap();
    assert!(matches!(
        ModPack::load(std::slice::from_ref(&first)),
        Err(ModError::Materials(MaterialError::UnknownReference { .. }))
    ));
    fs::write(first.path.join("materials.ron"), fixed("grain", "")).unwrap();
    fs::write(
        first.path.join("reactions.ron"),
        r#"[(a: "grain", b: "unknown:thing", prob: 1, out_a: "empty", out_b: "empty")]"#,
    )
    .unwrap();
    assert!(matches!(
        ModPack::load(&[first]),
        Err(ModError::Reactions(ReactionError::UnknownMaterial(_)))
    ));
}

#[test]
fn script_rules_are_deterministic() {
    let dir = TestDir::new();
    let mod_dir = dir.add(
        "rule",
        "demo",
        &fixed("gem", ""),
        "[]",
        Some(
            r#"[(every: 2, x: 10, y: 10, from: "stone", into: "gem"),
               (every: 3, x: 11, y: 10, from: "stone", into: "demo:gem")]"#,
        ),
    );
    let pack = ModPack::load(std::slice::from_ref(&mod_dir)).unwrap();
    let mut disabled = mod_dir;
    disabled.script = false;
    assert!(ModPack::load(&[disabled]).unwrap().rules().is_empty());
    for seed in 1..=8 {
        let mut traces = Vec::new();
        for _ in 0..2 {
            let mut world = pack.new_world(seed);
            world.insert_chunk(ChunkPos::new(0, 0));
            fill(&mut world, 10, 10, 11, 10, '#');
            let previous = world.get(CellPos::new(10, 10));
            let mut trace = Vec::new();
            for tick in 0..32 {
                pack.apply_rules(&mut world).unwrap();
                if tick == 0 {
                    let changed = world.get(CellPos::new(10, 10));
                    assert_eq!(changed.material, pack.materials.id("demo:gem").unwrap());
                    assert_eq!((changed.temp, changed.shade), (previous.temp, previous.shade));
                }
                world.step();
                trace.push(world.state_hash());
            }
            assert_eq!(
                world.get(CellPos::new(11, 10)).material,
                pack.materials.id("demo:gem").unwrap()
            );
            assert_sleeps(&mut world, 1_000);
            traces.push(trace);
        }
        assert_eq!(traces[0], traces[1], "seed={seed}");
    }
}

#[test]
fn script_runs_by_mod_id_then_file_order() {
    let dir = TestDir::new();
    let first_dir = dir.add(
        "a-folder",
        "z",
        "[]",
        "[]",
        Some(r#"[(every: 1, x: 3, y: 3, from: "a:crystal", into: "stone")]"#),
    );
    let second_dir = dir.add(
        "z-folder",
        "a",
        &fixed("crystal", ""),
        "[]",
        Some(r#"[(every: 1, x: 3, y: 3, from: "stone", into: "crystal")]"#),
    );
    let pack = ModPack::load(&[first_dir, second_dir]).unwrap();
    for seed in 1..=8 {
        let mut world = pack.new_world(seed);
        world.insert_chunk(ChunkPos::new(0, 0));
        fill(&mut world, 3, 3, 3, 3, '#');
        pack.apply_rules(&mut world).unwrap();
        assert_eq!(
            world.get(CellPos::new(3, 3)).material,
            pack.materials.id("stone").unwrap(),
            "seed={seed}"
        );
        assert_sleeps(&mut world, 1_000);
    }
}

#[test]
fn script_rule_limit_is_enforced() {
    let dir = TestDir::new();
    let mut script = String::from("[");
    for _ in 0..1024 {
        script.push_str("(every: 1, x: 2, y: 2, from: \"stone\", into: \"stone\"),");
    }
    script.push_str("(every: 1, x: 2, y: 2, from: \"stone\", into: \"limit:gem\")]");
    let mod_dir = dir.add("limit", "limit", &fixed("gem", ""), "[]", Some(&script));
    let pack = ModPack::load(&[mod_dir]).unwrap();
    for seed in 1..=8 {
        let mut world = pack.new_world(seed);
        world.insert_chunk(ChunkPos::new(0, 0));
        fill(&mut world, 2, 2, 2, 2, '#');
        let before = world.get(CellPos::new(2, 2));
        assert!(matches!(pack.apply_rules(&mut world), Err(ModError::RuleLimit)), "seed={seed}");
        assert_eq!(world.get(CellPos::new(2, 2)), before, "seed={seed}: 1025 件目が実行された");
        assert_sleeps(&mut world, 1_000);
    }
}

#[test]
fn manifest_and_opt_in_are_validated() {
    let dir = TestDir::new();
    let mod_dir = dir.add("one", "x", "[]", "[]", None);
    fs::write(mod_dir.path.join("manifest.ron"), "(id: \"bad:id\", version: 1)").unwrap();
    assert!(matches!(
        ModPack::load(std::slice::from_ref(&mod_dir)),
        Err(ModError::InvalidManifest(_))
    ));
    fs::write(mod_dir.path.join("manifest.ron"), "(id: \"good\", version: 2)").unwrap();
    assert!(matches!(
        ModPack::load(std::slice::from_ref(&mod_dir)),
        Err(ModError::InvalidManifest(_))
    ));
    fs::write(mod_dir.path.join("manifest.ron"), "(id: \"good\", version: 1)").unwrap();
    assert!(ModPack::load(std::slice::from_ref(&mod_dir)).is_ok());
    assert!(!Path::new(&mod_dir.path.join("script.ron")).exists());
    let mut opted_in = mod_dir;
    opted_in.script = true;
    assert!(matches!(ModPack::load(&[opted_in]), Err(ModError::Read { .. })));
}
