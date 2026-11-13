//! 仕様 09 §6: 参照デモの組み立て。MOD の読み込み、生成器、剛体、起動。

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use charon_core::cell::{Cell, MaterialId};
use charon_core::world::World;
use charon_game::mods::{ModDirectory, ModPack};
use charon_physics::{BodyRender, PhysicsEngine};
use charon_render::DeepBackgroundParams;
use charon_world::demo::{DemoGenerator, DemoMaterials, PLAYER_SPAWN};
use charon_world::save::WorldMeta;
use charon_world::streaming::Streamer;
use winit::event_loop::EventLoop;

use crate::{
    AppOptions, DemoSession, EnemySpawn, PlayerLoadout, RestartFn, run_with_streaming_options,
};

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
        // 配布物: 実行ファイルと同じ場所の `assets` (どこから起動しても見つかるように)。
        let beside_exe = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("assets/mods/fse_demo")));
        let mod_dir = if local.join("manifest.ron").exists() {
            local
        } else if let Some(dir) = beside_exe.filter(|dir| dir.join("manifest.ron").exists()) {
            dir
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods/fse_demo")
        };
        Self { mod_dir, save_base: PathBuf::from("saves/demo") }
    }
}

/// デモ (ゲーム側) のプレイヤーの装備 (仕様 10 §7.4・§9.5)。左手の武器は `fse_demo` の `wands.ron` の杖、
/// 右手の道具はエンジンの既定のワイヤーフック。
pub(crate) fn demo_loadout() -> PlayerLoadout {
    PlayerLoadout {
        max_hp: 100,
        wand: "fse_demo:starter".to_owned(),
        hook: Some("grapple".to_owned()),
    }
}

/// デモ (ゲーム側) のキャラクターの画像: 32×64 の RGBA (魔法使い。右向き)。当たり判定 (5×10 セル) より大きく作り、
/// `scale = 10 / 64` で足元の中央を基準に 5×10 セルに描く。`Character scale` (Debug パネル) で大きさを変えられる。
pub(crate) fn demo_character_image() -> crate::SpriteAsset {
    const W: usize = 32;
    const H: usize = 64;
    let mut rgba = vec![0u8; W * H * 4];
    let mut put = |x: i32, y: i32, c: [u8; 4]| {
        if (0..W as i32).contains(&x) && (0..H as i32).contains(&y) {
            let at = (y as usize * W + x as usize) * 4;
            rgba[at..at + 4].copy_from_slice(&c);
        }
    };
    let ellipse = |cx: f32, cy: f32, rx: f32, ry: f32, x: i32, y: i32| {
        let (dx, dy) = ((x as f32 + 0.5 - cx) / rx, (y as f32 + 0.5 - cy) / ry);
        dx * dx + dy * dy <= 1.0
    };
    let robe = [96, 64, 190, 255];
    let robe_dark = [66, 44, 140, 255];
    let skin = [236, 190, 150, 255];
    let hat = [70, 48, 150, 255];
    let boots = [50, 36, 30, 255];
    let wood = [140, 96, 50, 255];
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            // 帽子 (つば + とんがり)。
            if ellipse(16.0, 15.0, 12.0, 3.0, x, y)
                || (y < 14 && (x as f32 - 16.0).abs() < (y as f32) * 0.55 && y > 0)
            {
                put(x, y, hat);
            }
            // 顔。
            if ellipse(16.0, 21.0, 6.5, 6.5, x, y) {
                put(x, y, skin);
            }
            // ローブ (裾が広がる)。
            let half = 6.0 + (y as f32 - 28.0).max(0.0) * 0.28;
            if (28..58).contains(&y) && (x as f32 + 0.5 - 16.0).abs() < half {
                put(x, y, if (x + y) % 7 == 0 { robe_dark } else { robe });
            }
            // 足 (ブーツ)。
            if (58..64).contains(&y) && ((6..14).contains(&x) || (18..27).contains(&x)) {
                put(x, y, boots);
            }
            // 杖 (右手)。
            if (25..60).contains(&y) && (x == 27 || x == 28) {
                put(x, y, wood);
            }
        }
    }
    // 目と杖の先の光。
    for (x, y) in [(14, 21), (19, 21)] {
        put(x, y, [30, 30, 40, 255]);
    }
    for y in 21..26 {
        for x in 25..31 {
            if ellipse(27.5, 23.5, 3.0, 3.0, x, y) {
                put(x, y, [120, 230, 255, 255]);
            }
        }
    }
    crate::SpriteAsset {
        width: W as u16,
        height: H as u16,
        rgba: rgba.into(),
        pivot: [W as f32 / 2.0, H as f32],
        scale: 10.0 / H as f32,
        filter: charon_render::SpriteFilter::Nearest,
    }
}

/// デモ (ゲーム側) の、背景のさらに奥で動く虹色の塊 (仕様 09 §9)。配置は世界の seed で決まる。
/// 空の色は、シーンの背景色 (`DEMO_CLEAR`) と同じにする。
pub(crate) fn demo_deep_background(seed: u64) -> DeepBackgroundParams {
    DeepBackgroundParams {
        seed: (seed as u32) ^ 0x5241_494E,
        parallax: 0.5,
        grid: 90.0,
        density: 0.55,
        min_radius: 6.0,
        max_radius: 16.0,
        drift: 0.02,
        roam: 0.3,
        brightness: 1.0,
        empty: [crate::DEMO_CLEAR.r as f32, crate::DEMO_CLEAR.g as f32, crate::DEMO_CLEAR.b as f32],
    }
}

/// デモ (ゲーム側) が部屋に置く敵 (仕様 10 §8.5)。種類はエンジンの既定の定義。
pub(crate) fn demo_enemies() -> Vec<EnemySpawn> {
    vec![
        EnemySpawn { name: "walker".to_owned(), cell: [164, 248] },
        EnemySpawn { name: "shooter".to_owned(), cell: [176, 248] },
    ]
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

/// 十字 (`X`) の回る速さ (ラジアン / 秒)。約 2 秒で 1 周する。
pub(crate) const CROSS_SPIN: f32 = 3.0;

/// 十字の剛体 (16×16、`ice` だけ) を、その場でくるくる回す (ゲーム側の指定、仕様 09 §6・仕様 05 §11)。
/// 1 つ目の十字は**ドット**で `+CROSS_SPIN`、2 つ目の十字は**滑らか**で逆向き (`-CROSS_SPIN`)。
/// 保存・読み込みは回転と描き方の指定を持たないので、新しい世界でも読み込んだ世界でも呼ぶ。
pub(crate) fn spin_crosses(physics: &mut PhysicsEngine, ice: MaterialId) {
    let crosses: Vec<usize> = physics
        .bodies
        .iter()
        .enumerate()
        .filter(|(_, body)| {
            body.width == 16
                && body.height == 16
                && body
                    .cells
                    .iter()
                    .all(|cell| cell.material == MaterialId::EMPTY || cell.material == ice)
                && body.cells.iter().any(|cell| cell.material == ice)
        })
        .map(|(index, _)| index)
        .collect();
    if let Some(&dots) = crosses.first() {
        physics.set_render(dots, BodyRender::Dots);
        physics.set_spin(dots, Some(CROSS_SPIN));
    }
    if let Some(&smooth) = crosses.get(1) {
        physics.set_render(smooth, BodyRender::Smooth);
        physics.set_spin(smooth, Some(-CROSS_SPIN));
    }
}

/// 部屋に置く剛体 4 つ (仕様 09 §6、仕様 05 §11): ドットの X 字 (`ice`、空中でくるくる回る)、床に置く輪 (`wood`)・Z 字
/// (`stone`)、そして**滑らか**な X 字 (`ice`、上の方で逆向きに回る)。
pub fn demo_bodies(world: &World) -> Result<PhysicsEngine, Box<dyn Error>> {
    let materials = world.materials();
    let id = |name: &str| materials.id(name).ok_or_else(|| format!("材質 {name} がありません"));
    let (ice, wood, stone) = (id("ice")?, id("wood")?, id("stone")?);
    let mut physics = PhysicsEngine::new();
    let x_cells = shape(16, 16, ice, |x, y| (x - y).abs() <= 2 || (x + y - 15).abs() <= 2);
    // 回転の半径 (対角線の半分、約 11.3 セル) が床 (y = 264) に当たらない高さに浮かせる。
    physics.create_body(16, 16, x_cells, materials, [80.0, 250.0])?;
    let ring = shape(19, 19, wood, |x, y| {
        let distance = (x - 9) * (x - 9) + (y - 9) * (y - 9);
        (36..=81).contains(&distance)
    });
    physics.create_body(19, 19, ring, materials, [112.0, 244.0])?;
    let z_cells = shape(16, 16, stone, |x, y| y <= 2 || y >= 13 || (x + y - 15).abs() <= 1);
    physics.create_body(16, 16, z_cells, materials, [144.0, 246.0])?;
    // 滑らかな十字: ドットの十字と同じ形。輪とドットの十字に触れない、上の方に浮かせる。
    let smooth_cells = shape(16, 16, ice, |x, y| (x - y).abs() <= 2 || (x + y - 15).abs() <= 2);
    physics.create_body(16, 16, smooth_cells, materials, [60.0, 226.0])?;
    spin_crosses(&mut physics, ice);
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
        Some(mut physics) => {
            // 保存した十字は、読み込んだあとも回し続ける。
            if let Some(ice) = materials.id("ice") {
                spin_crosses(&mut physics, ice);
            }
            physics
        }
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
    let metadata_seed = metadata.seed;
    Ok(OpenDemo {
        session: DemoSession { world, streamer, save_root: root, metadata, physics },
        options: AppOptions {
            follow_player: true,
            player_spawn: Some(PLAYER_SPAWN),
            rainbow_sand: Some(demo_materials.rainbow_sand),
            demo: true,
            background: Some(DemoGenerator { materials: demo_materials }),
            definitions: Some(Arc::new(pack.definitions.clone())),
            player: Some(demo_loadout()),
            deep_background: Some(demo_deep_background(metadata_seed)),
            enemies: demo_enemies(),
            player_sprite: Some(demo_character_image()),
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
    run_demo_with(EventLoop::new()?, config)
}

/// [`run_demo`] に、呼び出し側が作ったイベントループを渡す (Android は専用のループが要る)。
pub fn run_demo_with(event_loop: EventLoop<()>, config: DemoConfig) -> Result<(), Box<dyn Error>> {
    let OpenDemo { session, options, brush } = open_demo(&config, 1)?;
    let DemoSession { world, streamer, save_root, metadata, physics } = session;
    let restart_config = config.clone();
    let restart: RestartFn =
        Box::new(move |seed| open_demo(&restart_config, seed).map(|open| open.session));
    run_with_streaming_options(
        event_loop,
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
    use charon_game::game::{GameInput, GameState};
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
        let mut enemy_spawns = options.enemies.clone();
        let mut game = GameState::from_definitions(options.definitions.as_ref().unwrap());
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
                game: &mut game,
                definitions: options.definitions.as_deref(),
                loadout: options.player.as_ref(),
                enemy_spawns: &mut enemy_spawns,
            };
            sim.tick(
                ChunkPos::new(0, 3),
                PlayerInput::default(),
                GameInput::default(),
                &mut physics_ms,
            )
            .unwrap();
        }
        let mut hash = world.state_hash().rotate_left(7) ^ entities.state_hash();
        for body in &physics.bodies {
            let t = physics.world.bodies[body.handle].translation();
            hash =
                hash.rotate_left(5) ^ u64::from(t.x.to_bits()) ^ (u64::from(t.y.to_bits()) << 32);
        }
        // 床に立っているのはプレイヤー。敵は徘徊・跳躍で、その tick に空中にいることがある。
        let grounded = player.and_then(|id| entities.get(id)).is_some_and(|e| e.grounded);
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
            // プレイヤー 1 人と、ゲーム側が指定した敵が生成されて床に立ち、剛体 3 つがスタンプされている。
            assert_eq!(first.1, 1 + demo_enemies().len(), "seed {seed}: プレイヤーと敵");
            assert!(first.2, "seed {seed}: 接地");
            assert!(first.3 > 100, "seed {seed}: 剛体のスタンプ {} セル", first.3);
        }
        // seed が違えば洞窟が違うので、世界のハッシュも違う。
        assert_ne!(run(1, 150).0, run(2, 150).0);
    }

    /// 仕様 05 §11: 2 つ目の十字は**滑らか**で、逆向きに回り、スタンプしたセルに `FLAG_SMOOTH` が付く。
    /// 1 つ目の十字・輪・Z 字は従来どおりドットで、`FLAG_SMOOTH` は付かない (画面なし)。
    #[test]
    fn demo_has_a_smooth_cross() {
        use charon_core::cell::{FLAG_BODY, FLAG_SMOOTH};
        for seed in 1..=3 {
            let dir = TestDir::new();
            let OpenDemo { session, options, .. } = open_demo(&config(&dir), seed).unwrap();
            let DemoSession { mut world, streamer, save_root, metadata, mut physics } = session;
            let mut streaming = Some(StreamingState { streamer, save_root, metadata });
            let mut initialized = false;
            let mut entities = EntityWorld::new();
            let mut player = None;
            let mut spawn = options.player_spawn;
            let mut enemy_spawns = Vec::new();
            let mut game = GameState::from_definitions(options.definitions.as_ref().unwrap());
            let mut physics_ms = 0.0;
            for _ in 0..200 {
                Sim {
                    world: &mut world,
                    streaming: &mut streaming,
                    physics: &mut physics,
                    initialized: &mut initialized,
                    entities: &mut entities,
                    player: &mut player,
                    spawn: &mut spawn,
                    game: &mut game,
                    definitions: options.definitions.as_deref(),
                    loadout: options.player.as_ref(),
                    enemy_spawns: &mut enemy_spawns,
                }
                .tick(
                    ChunkPos::new(0, 3),
                    PlayerInput::default(),
                    GameInput::default(),
                    &mut physics_ms,
                )
                .unwrap();
            }
            let renders: Vec<BodyRender> = physics.bodies.iter().map(|b| b.render).collect();
            assert_eq!(
                renders,
                [BodyRender::Dots, BodyRender::Dots, BodyRender::Dots, BodyRender::Smooth],
                "seed {seed}"
            );
            assert_eq!(physics.bodies[0].spin, Some(CROSS_SPIN));
            assert_eq!(physics.bodies[3].spin, Some(-CROSS_SPIN), "逆向きに回る");
            let flags = |index: usize| -> (usize, usize, usize) {
                let stamped = &physics.bodies[index].stamped;
                let body = stamped.iter().filter(|&&p| world.get(p).has(FLAG_BODY)).count();
                let smooth = stamped.iter().filter(|&&p| world.get(p).has(FLAG_SMOOTH)).count();
                (stamped.len(), body, smooth)
            };
            let (len, body, smooth) = flags(3);
            assert!(
                len > 80 && body == len && smooth == len,
                "seed {seed}: 滑らかな十字 {len}/{body}/{smooth}"
            );
            for index in 0..3 {
                let (len, body, smooth) = flags(index);
                assert!(
                    len > 20 && body == len && smooth == 0,
                    "seed {seed}: 物体 {index} はドット"
                );
            }
            streaming.take().unwrap().streamer.finish().unwrap();
        }
    }

    /// 部屋の十字 (`X`) は、その場でくるくる回り続ける。輪と Z 字は回らない (動的な剛体のまま)。
    #[test]
    fn demo_cross_spins() {
        let wrap = |delta: f32| {
            let tau = std::f32::consts::TAU;
            let d = delta.rem_euclid(tau);
            if d > std::f32::consts::PI { d - tau } else { d }
        };
        for seed in 1..=3 {
            let dir = TestDir::new();
            let OpenDemo { session, options, .. } = open_demo(&config(&dir), seed).unwrap();
            let DemoSession { mut world, streamer, save_root, metadata, mut physics } = session;
            // 十字だけが回る指定になっている。
            let spinning: Vec<bool> = physics.bodies.iter().map(|b| b.spin.is_some()).collect();
            assert_eq!(spinning, [true, false, false, true], "seed {seed}");
            let mut streaming = Some(StreamingState { streamer, save_root, metadata });
            let mut initialized = false;
            let mut entities = EntityWorld::new();
            let mut player = None;
            let mut spawn = options.player_spawn;
            let mut enemy_spawns = Vec::new();
            let mut game = GameState::from_definitions(options.definitions.as_ref().unwrap());
            let mut physics_ms = 0.0;
            let cross = |physics: &PhysicsEngine| {
                let rigid = &physics.world.bodies[physics.bodies[0].handle];
                let t = rigid.translation();
                ([t.x, t.y], rigid.rotation().angle())
            };
            let (mut start, mut angle, mut turned) = (None, 0.0_f32, 0.0_f32);
            for tick in 0..300 {
                Sim {
                    world: &mut world,
                    streaming: &mut streaming,
                    physics: &mut physics,
                    initialized: &mut initialized,
                    entities: &mut entities,
                    player: &mut player,
                    spawn: &mut spawn,
                    game: &mut game,
                    definitions: options.definitions.as_deref(),
                    loadout: options.player.as_ref(),
                    enemy_spawns: &mut enemy_spawns,
                }
                .tick(
                    ChunkPos::new(0, 3),
                    PlayerInput::default(),
                    GameInput::default(),
                    &mut physics_ms,
                )
                .unwrap();
                let (position, now) = cross(&physics);
                if tick == 150 {
                    start = Some(position);
                    angle = now;
                }
                if tick > 150 {
                    turned += wrap(now - angle);
                    angle = now;
                    let start = start.unwrap();
                    assert!(
                        (position[0] - start[0]).abs() < 0.01
                            && (position[1] - start[1]).abs() < 0.01,
                        "seed {seed}: 回りながら位置は動かない"
                    );
                }
            }
            // 149 tick (約 2.5 秒) で、3 ラジアン / 秒 → 約 7.5 ラジアン (1 周以上) 回る。
            assert!((turned - CROSS_SPIN * 149.0 / 60.0).abs() < 0.4, "seed {seed}: {turned}");
            streaming.take().unwrap().streamer.finish().unwrap();
        }
    }

    /// 仕様 10 §8.5: ゲーム側が指定した敵が生まれ、プレイヤーを狙い、決定的に動く (画面なし)。
    #[test]
    fn demo_enemies_spawn_in_scene() {
        let run = |seed: u64| {
            let dir = TestDir::new();
            let OpenDemo { session, options, .. } = open_demo(&config(&dir), seed).unwrap();
            let DemoSession { mut world, streamer, save_root, metadata, mut physics } = session;
            let mut streaming = Some(StreamingState { streamer, save_root, metadata });
            let mut initialized = false;
            let mut entities = EntityWorld::new();
            let mut player = None;
            let mut spawn = options.player_spawn;
            let mut enemy_spawns = options.enemies.clone();
            let mut game = GameState::from_definitions(options.definitions.as_ref().unwrap());
            let mut physics_ms = 0.0;
            let mut trace = Vec::new();
            for _ in 0..400 {
                Sim {
                    world: &mut world,
                    streaming: &mut streaming,
                    physics: &mut physics,
                    initialized: &mut initialized,
                    entities: &mut entities,
                    player: &mut player,
                    spawn: &mut spawn,
                    game: &mut game,
                    definitions: options.definitions.as_deref(),
                    loadout: options.player.as_ref(),
                    enemy_spawns: &mut enemy_spawns,
                }
                .tick(
                    ChunkPos::new(0, 3),
                    PlayerInput::default(),
                    GameInput::default(),
                    &mut physics_ms,
                )
                .unwrap();
                trace.push(game.state_hash() ^ entities.state_hash().rotate_left(13));
            }
            let names: Vec<String> = game.enemies.values().map(|e| e.def.name.clone()).collect();
            let hp = game.vitals.get(player.unwrap()).unwrap().hp;
            streaming.take().unwrap().streamer.finish().unwrap();
            (trace, names, hp, enemy_spawns.len())
        };
        for seed in 1..=3 {
            let (trace, names, hp, pending) = run(seed);
            assert_eq!(names, ["walker", "shooter"], "seed {seed}: ゲーム側の指定どおり");
            assert_eq!(pending, 0, "seed {seed}: 全部生成された");
            assert!(hp <= 100);
            assert_eq!(trace, run(seed).0, "seed {seed}: 決定的");
        }
    }

    /// 仕様 10 §9.5: デモのプレイヤーは右手にワイヤーフックを持って始まり、右ボタン相当の入力 (`tool`) で
    /// 部屋の天井へワイヤーを撃って引かれる。左ボタン相当の入力 (`fire`) の杖とは同時に使える (画面なし)。
    #[test]
    fn demo_hook_in_scene() {
        for seed in 1..=3 {
            let dir = TestDir::new();
            let OpenDemo { session, options, .. } = open_demo(&config(&dir), seed).unwrap();
            let DemoSession { mut world, streamer, save_root, metadata, mut physics } = session;
            let mut streaming = Some(StreamingState { streamer, save_root, metadata });
            let mut initialized = false;
            let mut entities = EntityWorld::new();
            let mut player = None;
            let mut spawn = options.player_spawn;
            let mut enemy_spawns = Vec::new();
            let mut game = GameState::from_definitions(options.definitions.as_ref().unwrap());
            let mut physics_ms = 0.0;
            let (mut anchored_at, mut start_y, mut shots) = (None, None, 0);
            let mut top = i64::MAX;
            for tick in 0..300 {
                // 床に立ってから、天井へ向けて右ボタンを押し続け、左ボタンも押す。
                let action = if tick >= 150 {
                    GameInput { fire: true, tool: true, aim: (16, -48) }
                } else {
                    GameInput::default()
                };
                Sim {
                    world: &mut world,
                    streaming: &mut streaming,
                    physics: &mut physics,
                    initialized: &mut initialized,
                    entities: &mut entities,
                    player: &mut player,
                    spawn: &mut spawn,
                    game: &mut game,
                    definitions: options.definitions.as_deref(),
                    loadout: options.player.as_ref(),
                    enemy_spawns: &mut enemy_spawns,
                }
                .tick(ChunkPos::new(0, 3), PlayerInput::default(), action, &mut physics_ms)
                .unwrap();
                // プレイヤーは周囲のチャンクが読み込まれてから生まれる。
                let Some(id) = player else { continue };
                let e = entities.get(id).unwrap();
                if tick == 149 {
                    start_y = Some(e.y16);
                    assert!(
                        game.hooks.contains_key(&id),
                        "seed {seed}: 右手にフックを持って始まる"
                    );
                    assert!(game.rope(id).is_none());
                }
                if anchored_at.is_none() && game.rope(id).and_then(|r| r.anchor()).is_some() {
                    anchored_at = Some(tick);
                }
                if tick >= 150 {
                    top = top.min(e.y16);
                }
                shots = shots.max(game.projectiles.len());
            }
            let anchored_at = anchored_at.expect("ワイヤーが天井に着く");
            assert!((150..=160).contains(&anchored_at), "seed {seed}: tick {anchored_at}");
            let start_y = start_y.unwrap();
            assert!(
                start_y - top > 5 * 16,
                "seed {seed}: 引かれて上がる ({} セル)",
                (start_y - top) / 16
            );
            assert!(shots > 0, "seed {seed}: 武器 (左手) も同時に撃てる");
            streaming.take().unwrap().streamer.finish().unwrap();
        }
    }

    /// 仕様 10 §7: デモ (ゲーム側) の杖と呪文は `fse_demo` の RON から来て、エンジン既定の `bomb` を上書きする。
    #[test]
    fn demo_definitions_come_from_the_game_files() {
        use charon_game::defs::Definitions;
        use charon_game::projectile::Payload;
        use charon_game::wand::SpellId;

        let dir = TestDir::new();
        let open = open_demo(&config(&dir), 1).unwrap();
        let defs = open.options.definitions.as_deref().unwrap();
        let loadout = open.options.player.as_ref().unwrap();
        assert_eq!(loadout, &demo_loadout());

        // エンジンの既定は半径 6、デモのファイルが 8 に上書きする。位置 (SpellId) は同じ。
        let engine = Definitions::engine_defaults(open.session.world.materials()).unwrap();
        let radius = |defs: &Definitions| match defs.spells.get(SpellId(1)).unwrap().payload {
            Payload::Explode { radius } => radius,
            other => panic!("bomb が爆発ではない: {other:?}"),
        };
        assert_eq!(radius(&engine), 6);
        assert_eq!(radius(defs), 8);
        assert_eq!(defs.spells.id("bomb"), Some(SpellId(1)));
        // ゲームの追加は名前空間付き。杖はデモの呪文を先に探す。
        assert!(defs.spells.id("fse_demo:rainbow_shot").is_some());
        let starter = defs.wand_def(&loadout.wand).unwrap();
        assert_eq!(starter.spells, ["spark", "fse_demo:rainbow_shot", "bomb"]);
        let wand = defs.wand(&loadout.wand).unwrap();
        assert_eq!(wand.spells.len(), 3);
        // エンジンの既定の杖・敵も残っている。
        assert!(defs.wand("basic").is_ok() && defs.enemy("shooter").is_some());
    }

    /// 仕様 10 §6.3: 右ボタン相当の入力で弾が出て、爆発のセルがパーティクルになる (画面なし)。
    #[test]
    fn demo_wand_fires_in_scene() {
        for seed in 1..=3 {
            let dir = TestDir::new();
            let OpenDemo { session, options, .. } = open_demo(&config(&dir), seed).unwrap();
            let DemoSession { mut world, streamer, save_root, metadata, mut physics } = session;
            let mut streaming = Some(StreamingState { streamer, save_root, metadata });
            let mut initialized = false;
            let mut entities = EntityWorld::new();
            let mut player = None;
            let mut spawn = options.player_spawn;
            let mut enemy_spawns = options.enemies.clone();
            let mut game = GameState::from_definitions(options.definitions.as_ref().unwrap());
            let mut physics_ms = 0.0;
            let (mut shots, mut max_particles) = (0, 0);
            for tick in 0..260 {
                // プレイヤーが床に立ってから、右へ撃ち続ける。
                let action = if tick >= 150 {
                    GameInput { fire: true, aim: (16, 0), ..Default::default() }
                } else {
                    GameInput::default()
                };
                Sim {
                    world: &mut world,
                    streaming: &mut streaming,
                    physics: &mut physics,
                    initialized: &mut initialized,
                    entities: &mut entities,
                    player: &mut player,
                    spawn: &mut spawn,
                    game: &mut game,
                    definitions: options.definitions.as_deref(),
                    loadout: options.player.as_ref(),
                    enemy_spawns: &mut enemy_spawns,
                }
                .tick(ChunkPos::new(0, 3), PlayerInput::default(), action, &mut physics_ms)
                .unwrap();
                shots = shots.max(game.projectiles.len());
                max_particles = max_particles.max(physics.particles().len());
            }
            let id = player.unwrap();
            assert!(game.wands.contains_key(&id), "seed {seed}: プレイヤーが杖を持つ");
            assert!(game.vitals.get(id).is_some(), "seed {seed}: プレイヤーが体力を持つ");
            assert!(shots > 0, "seed {seed}: 弾が飛ぶ");
            assert!(max_particles > 0, "seed {seed}: 爆発でセルがパーティクルになる");
            streaming.take().unwrap().streamer.finish().unwrap();
        }
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
        assert_eq!(physics.bodies.len(), 4);
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
