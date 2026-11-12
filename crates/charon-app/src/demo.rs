//! 仕様 09 §6: 参照デモの組み立て。MOD の読み込み、生成器、剛体、起動。

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use charon_core::cell::{Cell, MaterialId};
use charon_core::world::World;
use charon_game::mods::{ModDirectory, ModPack};
use charon_physics::PhysicsEngine;
use charon_world::demo::{DemoGenerator, DemoMaterials, PLAYER_SPAWN};
use charon_world::save::WorldMeta;
use charon_world::streaming::Streamer;
use winit::event_loop::EventLoop;

use crate::{AppOptions, DemoSession, RestartFn, run_with_streaming_options};

/// デモの MOD と保存先の場所。
#[derive(Clone, Debug)]
pub struct DemoConfig {
    /// `manifest.ron` を含む `fse_demo` のディレクトリ。
    pub mod_dir: PathBuf,
    /// seed ごとの保存先の親。実際の保存先は `save_base/seed-N`。
    pub save_base: PathBuf,
}

impl DemoConfig {
    /// 起動した場所の `assets/mods/fse_demo` があればそれを、なければこのリポジトリの `assets` を使う。
    /// 保存先は起動した場所の `saves/demo`。
    pub fn discover() -> Self {
        let local = PathBuf::from("assets/mods/fse_demo");
        let mod_dir = if local.join("manifest.ron").exists() {
            local
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods/fse_demo")
        };
        Self { mod_dir, save_base: PathBuf::from("saves/demo") }
    }
}

/// 組み立てたデモ。`session` を `run_with_streaming_options` に渡す。
#[derive(Debug)]
pub struct OpenDemo {
    pub session: DemoSession,
    pub options: AppOptions,
    /// 起動時のブラシの材質 (虹色の砂の先頭)。
    pub brush: MaterialId,
}

/// 5 セルの線の太さで描く 16×16 の `X`、外径 19 の輪、`Z` の形。`true` のセルが材質になる。
fn shape(
    width: usize,
    height: usize,
    material: MaterialId,
    inside: impl Fn(i32, i32) -> bool,
) -> Vec<Cell> {
    let mut cells = vec![Cell::default(); width * height];
    for y in 0..height {
        for x in 0..width {
            if inside(x as i32, y as i32) {
                cells[y * width + x].material = material;
            }
        }
    }
    cells
}

/// 部屋の床に置く X 字 (`ice`)、輪 (`wood`)、Z 字 (`stone`) の剛体 3 つ (仕様 09 §6)。
pub fn demo_bodies(world: &World) -> Result<PhysicsEngine, Box<dyn Error>> {
    let materials = world.materials();
    let id = |name: &str| materials.id(name).ok_or_else(|| format!("材質 {name} がありません"));
    let (ice, wood, stone) = (id("ice")?, id("wood")?, id("stone")?);
    let mut physics = PhysicsEngine::new();
    let x_cells = shape(16, 16, ice, |x, y| (x - y).abs() <= 1 || (x + y - 15).abs() <= 1);
    physics.create_body(16, 16, x_cells, materials, [80.0, 246.0])?;
    let ring = shape(19, 19, wood, |x, y| {
        let distance = (x - 9) * (x - 9) + (y - 9) * (y - 9);
        (36..=81).contains(&distance)
    });
    physics.create_body(19, 19, ring, materials, [112.0, 244.0])?;
    let z_cells = shape(16, 16, stone, |x, y| y <= 2 || y >= 13 || (x + y - 15).abs() <= 1);
    physics.create_body(16, 16, z_cells, materials, [144.0, 246.0])?;
    Ok(physics)
}

/// `seed` のデモを組み立てる。保存があればその seed・tick・剛体を読み込み、なければ新しく作る。
pub fn open_demo(config: &DemoConfig, seed: u64) -> Result<OpenDemo, Box<dyn Error>> {
    let pack = ModPack::load(&[ModDirectory { path: config.mod_dir.clone(), script: false }])?;
    let materials = Arc::clone(&pack.materials);
    let demo_materials =
        DemoMaterials::from_table(&materials).ok_or("fse_demo の材質が足りません")?;
    let root = config.save_base.join(format!("seed-{seed}"));
    let saved = WorldMeta::load(&root)?;
    let mut metadata = saved.clone().unwrap_or_else(|| WorldMeta::new(seed, &materials));
    for name in charon_world::material_names(&materials) {
        if !metadata.materials.contains(&name) {
            metadata.materials.push(name);
        }
    }
    let mut world = pack.new_world(metadata.seed);
    world.restore_tick(metadata.tick);
    let physics = match PhysicsEngine::load_bodies(&root, &materials, &metadata.materials)? {
        Some(physics) => physics,
        None => demo_bodies(&world)?,
    };
    let streamer = Streamer::with_saved_names(
        root.clone(),
        Arc::clone(&materials),
        metadata.seed,
        Arc::new(DemoGenerator { materials: demo_materials }),
        3,
        metadata.materials.clone(),
    )?;
    Ok(OpenDemo {
        session: DemoSession { world, streamer, save_root: root, metadata, physics },
        options: AppOptions {
            follow_player: true,
            player_spawn: Some(PLAYER_SPAWN),
            rainbow_sand: Some(demo_materials.rainbow_sand),
            demo: true,
        },
        brush: demo_materials.rainbow_sand,
    })
}

/// 再生記録用の世界。デモの UI には再生ボタンがなく、呼ばれない。
fn make_world(seed: u64) -> World {
    charon_core::testkit::world_from_ascii(&charon_core::testkit::box_art(16, 16), seed)
}

/// ウィンドウを開いてデモを動かす。`Main Menu` から別の seed に作り直せる。
pub fn run_demo(config: DemoConfig) -> Result<(), Box<dyn Error>> {
    let OpenDemo { session, options, brush } = open_demo(&config, 1)?;
    let DemoSession { world, streamer, save_root, metadata, physics } = session;
    let restart_config = config.clone();
    let restart: RestartFn =
        Box::new(move |seed| open_demo(&restart_config, seed).map(|open| open.session));
    run_with_streaming_options(
        EventLoop::new()?,
        world,
        brush,
        make_world,
        streamer,
        save_root,
        metadata,
        physics,
        options,
        Some(restart),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use charon_core::coords::ChunkPos;
    use charon_game::{EntityWorld, PlayerInput};

    use super::*;
    use crate::StreamingState;
    use crate::sim::Sim;

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            loop {
                let path = std::env::temp_dir().join(format!(
                    "charon-demo-{}-{}",
                    std::process::id(),
                    NEXT_DIR.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(err) => panic!("テストの保存先を作れません: {err}"),
                }
            }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn config(dir: &TestDir) -> DemoConfig {
        DemoConfig {
            mod_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods/fse_demo"),
            save_base: dir.0.clone(),
        }
    }

    /// 描画ループと同じ `Sim::tick` を `ticks` 回進め、世界・プレイヤー・剛体をまとめたハッシュと状態を返す。
    fn run(seed: u64, ticks: u32) -> (u64, usize, bool, usize) {
        let dir = TestDir::new();
        let OpenDemo { session, options, .. } = open_demo(&config(&dir), seed).unwrap();
        let DemoSession { mut world, streamer, save_root, metadata, mut physics } = session;
        let mut streaming = Some(StreamingState { streamer, save_root, metadata });
        let mut initialized = false;
        let mut entities = EntityWorld::new();
        let mut player = None;
        let mut spawn = options.player_spawn;
        let mut physics_ms = 0.0;
        for _ in 0..ticks {
            let mut sim = Sim {
                world: &mut world,
                streaming: &mut streaming,
                physics: &mut physics,
                initialized: &mut initialized,
                entities: &mut entities,
                player: &mut player,
                spawn: &mut spawn,
            };
            sim.tick(ChunkPos::new(0, 3), PlayerInput::default(), &mut physics_ms).unwrap();
        }
        let mut hash = world.state_hash().rotate_left(7) ^ entities.state_hash();
        for body in &physics.bodies {
            let t = physics.world.bodies[body.handle].translation();
            hash =
                hash.rotate_left(5) ^ u64::from(t.x.to_bits()) ^ (u64::from(t.y.to_bits()) << 32);
        }
        let grounded = entities.iter().all(|entity| entity.grounded);
        let stamped = physics.bodies.iter().map(|body| body.stamped.len()).sum();
        streaming.take().unwrap().streamer.finish().unwrap();
        (hash, entities.iter().count(), grounded, stamped)
    }

    #[test]
    fn demo_scene_is_deterministic() {
        for seed in 1..=8 {
            let first = run(seed, 150);
            let second = run(seed, 150);
            assert_eq!(first, second, "seed {seed}");
            // プレイヤーが 1 人生成されて床に立ち、剛体 3 つが世界にスタンプされている。
            assert_eq!(first.1, 1, "seed {seed}: プレイヤー");
            assert!(first.2, "seed {seed}: 接地");
            assert!(first.3 > 100, "seed {seed}: 剛体のスタンプ {} セル", first.3);
        }
        // seed が違えば洞窟が違うので、世界のハッシュも違う。
        assert_ne!(run(1, 150).0, run(2, 150).0);
    }

    #[test]
    fn demo_bodies_are_valid_and_inside_the_room() {
        let pack = ModPack::load(&[ModDirectory {
            path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods/fse_demo"),
            script: false,
        }])
        .unwrap();
        let world = pack.new_world(1);
        let physics = demo_bodies(&world).unwrap();
        assert_eq!(physics.bodies.len(), 3);
        let [x0, y0, x1, y1] = charon_world::demo::ROOM_INTERIOR;
        for body in &physics.bodies {
            let center =
                physics.world.bodies[body.handle].translation() * charon_physics::CELLS_PER_METER;
            let half = f32::from(body.width.max(body.height)) / 2.0;
            assert!(center.x - half > x0 as f32 && center.x + half < x1 as f32);
            assert!(center.y - half > y0 as f32 && center.y + half < y1 as f32);
        }
    }
}
