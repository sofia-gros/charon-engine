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
    /// 設定 (`settings.ron`、仕様 10 §14) のファイル。
    pub settings: PathBuf,
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
        Self {
            mod_dir,
            save_base: PathBuf::from("saves/demo"),
            settings: PathBuf::from("saves/settings.ron"),
        }
    }
}

/// デモ (ゲーム側) のプレイヤーの装備 (仕様 10 §7.4・§9.5)。左手の武器は `fse_demo` の `wands.ron` の杖、
/// 右手の道具はエンジンの既定のワイヤーフック。
pub(crate) fn demo_loadout() -> PlayerLoadout {
    PlayerLoadout {
        max_hp: 100,
        wand: "fse_demo:starter".to_owned(),
        hook: Some("grapple".to_owned()),
        dig: Some("laser".to_owned()),
    }
}

/// キャラクターの配色。
#[derive(Clone, Copy)]
struct Palette {
    robe: [u8; 4],
    robe_dark: [u8; 4],
    skin: [u8; 4],
    hat: [u8; 4],
    boots: [u8; 4],
    wood: [u8; 4],
    glow: [u8; 4],
}

/// シートの 1 コマの大きさ (画素)。当たり判定 (5×10 セル) より大きく作る。
const FRAME: [usize; 2] = [32, 64];

/// 1 コマ (32×64) を `(ox, oy)` から描く。`bob` は体の上下 (画素)、`stride` は足の開き (-1..=1)、`jump` は腕を上げる。
fn draw_figure(
    rgba: &mut [u8],
    sheet_width: usize,
    (ox, oy): (usize, usize),
    palette: Palette,
    bob: i32,
    stride: i32,
    jump: bool,
) {
    let (w, h) = (FRAME[0] as i32, FRAME[1] as i32);
    let mut put = |x: i32, y: i32, c: [u8; 4]| {
        if (0..w).contains(&x) && (0..h).contains(&y) {
            let at = ((oy + y as usize) * sheet_width + ox + x as usize) * 4;
            rgba[at..at + 4].copy_from_slice(&c);
        }
    };
    let ellipse = |cx: f32, cy: f32, rx: f32, ry: f32, x: i32, y: i32| {
        let (dx, dy) = ((x as f32 + 0.5 - cx) / rx, (y as f32 + 0.5 - cy) / ry);
        dx * dx + dy * dy <= 1.0
    };
    let up = if jump { 6 } else { 0 };
    for y in 0..h {
        for x in 0..w {
            let yb = y - bob; // 体 (足以外) は bob だけ動く。
            // 帽子 (つば + とんがり)。
            if ellipse(16.0, 15.0, 12.0, 3.0, x, yb)
                || (yb < 14 && (x as f32 - 16.0).abs() < (yb as f32) * 0.55 && yb > 0)
            {
                put(x, y, palette.hat);
            }
            // 顔。
            if ellipse(16.0, 21.0, 6.5, 6.5, x, yb) {
                put(x, y, palette.skin);
            }
            // ローブ (裾が広がる)。
            let half = 6.0 + (yb as f32 - 28.0).max(0.0) * 0.28;
            if (28..58).contains(&yb) && (x as f32 + 0.5 - 16.0).abs() < half {
                put(x, y, if (x + y) % 7 == 0 { palette.robe_dark } else { palette.robe });
            }
            // 足 (ブーツ)。歩くと開く。
            let left = (6 - stride * 3..14 - stride * 3).contains(&x);
            let right = (18 + stride * 3..27 + stride * 3).contains(&x);
            if (58..64).contains(&y) && (left || right) {
                put(x, y, palette.boots);
            }
            // 杖 (右手)。ジャンプでは上げる。
            if (25 - up..60 - up).contains(&yb) && (x == 27 || x == 28) {
                put(x, y, palette.wood);
            }
        }
    }
    // 目と杖の先の光。
    for (x, y) in [(14, 21 + bob), (19, 21 + bob)] {
        put(x, y, [30, 30, 40, 255]);
    }
    for y in 21..26 {
        for x in 25..31 {
            if ellipse(27.5, 23.5 - up as f32, 3.0, 3.0, x, y - bob) {
                put(x, y - up, palette.glow);
            }
        }
    }
}

/// 4 行のスプライトシートを作る: 立つ (2 コマ)・歩く (4 コマ)・跳ぶ・落ちる。1 コマは 32×64。
fn demo_sheet(palette: Palette, scale: f32) -> crate::SpriteAsset {
    use crate::{Clip, Clips};
    let (sheet_w, sheet_h) = (FRAME[0] * 4, FRAME[1] * 4);
    let mut rgba = vec![0u8; sheet_w * sheet_h * 4];
    let mut figure = |col: usize, row: usize, bob: i32, stride: i32, jump: bool| {
        draw_figure(
            &mut rgba,
            sheet_w,
            (col * FRAME[0], row * FRAME[1]),
            palette,
            bob,
            stride,
            jump,
        );
    };
    figure(0, 0, 0, 0, false); // 立つ
    figure(1, 0, 1, 0, false); // 立つ (息をする)
    for (col, (bob, stride)) in [(0, 0), (1, 1), (0, 0), (1, -1)].into_iter().enumerate() {
        figure(col, 1, bob, stride, false); // 歩く
    }
    figure(0, 2, 0, 1, true); // 跳ぶ
    figure(0, 3, 1, 0, false); // 落ちる
    crate::SpriteAsset {
        width: sheet_w as u16,
        height: sheet_h as u16,
        rgba: rgba.into(),
        frame_size: [FRAME[0] as u16, FRAME[1] as u16],
        pivot: [FRAME[0] as f32 / 2.0, FRAME[1] as f32],
        scale,
        filter: charon_render::SpriteFilter::Nearest,
        clips: Clips {
            idle: Clip { row: 0, frames: 2, ticks_per_frame: 30 },
            walk: Clip { row: 1, frames: 4, ticks_per_frame: 6 },
            jump: Clip::still(2),
            fall: Clip::still(3),
        },
    }
}

/// デモ (ゲーム側) のキャラクターの画像: 4 行のシート (魔法使い。右向き)。当たり判定 (5×10 セル) より大きい 32×64 のコマを、
/// `scale = 10 / 64` で足元の中央を基準に 5×10 セルに描く。`Character scale` (Debug パネル) で大きさを変えられる。
pub(crate) fn demo_character_image() -> crate::SpriteAsset {
    demo_sheet(
        Palette {
            robe: [96, 64, 190, 255],
            robe_dark: [66, 44, 140, 255],
            skin: [236, 190, 150, 255],
            hat: [70, 48, 150, 255],
            boots: [50, 36, 30, 255],
            wood: [140, 96, 50, 255],
            glow: [120, 230, 255, 255],
        },
        10.0 / 64.0,
    )
}

/// デモ (ゲーム側) の敵の画像。敵の種類の名前 (`walker`・`shooter`) ごとに、配色を変えた同じ形。敵は 8×16 セル。
pub(crate) fn demo_enemy_images() -> std::collections::BTreeMap<String, crate::SpriteAsset> {
    let walker = Palette {
        robe: [70, 150, 70, 255],
        robe_dark: [40, 110, 50, 255],
        skin: [150, 200, 110, 255],
        hat: [90, 70, 40, 255],
        boots: [40, 30, 20, 255],
        wood: [110, 80, 50, 255],
        glow: [255, 230, 120, 255],
    };
    let shooter = Palette {
        robe: [190, 60, 60, 255],
        robe_dark: [130, 36, 40, 255],
        skin: [240, 170, 140, 255],
        hat: [120, 30, 40, 255],
        boots: [40, 24, 24, 255],
        wood: [120, 90, 60, 255],
        glow: [255, 140, 60, 255],
    };
    [("walker", walker), ("shooter", shooter)]
        .into_iter()
        .map(|(name, palette)| (name.to_owned(), demo_sheet(palette, 16.0 / 64.0)))
        .collect()
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
            enemy_sprites: demo_enemy_images(),
            settings_path: Some(config.settings.clone()),
            logic: Some(crate::api::GameLogic::new(DemoGame)),
        },
        brush: demo_materials.rainbow_sand,
    })
}

/// デモのゲームロジック (仕様 14): 出来事に、画面の揺れと音で反応する。エンジンの `Game` API の使用例。
#[derive(Clone, Copy, Debug)]
pub struct DemoGame;

impl crate::api::Game for DemoGame {
    fn on_tick(
        &mut self,
        ctx: &mut crate::api::Context<'_>,
        events: &[charon_game::game::GameEvent],
    ) {
        use charon_game::game::GameEvent;
        for event in events {
            match event {
                GameEvent::Defeated(_) => ctx.shake(0.5),
                GameEvent::Hit(hit) if hit.entity.is_some() => ctx.shake(0.15),
                _ => {}
            }
        }
    }
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
            settings: dir.0.join("settings.ron"),
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
                    GameInput { fire: true, tool: true, dig: false, aim: (16, -48) }
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

    /// 仕様 10 §12: デモのプレイヤーは掘る道具 (レーザー) を持って始まり、左の壁 (石、硬さ 600) へ向けて押し続けると、
    /// 力 6 で 100 tick 後に壁が消える。その前には消えない (画面なし)。
    #[test]
    fn demo_dig_in_scene() {
        use charon_core::coords::CellPos;
        for seed in 1..=2 {
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
            let mut started = None;
            let mut removed_at = None;
            for tick in 0..400 {
                let digging = tick >= 150;
                let action = GameInput { dig: digging, aim: (-1000, 0), ..Default::default() };
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
                let Some(id) = player else { continue };
                let e = entities.get(id).unwrap();
                let row = ((e.y16 + i64::from(e.height) * 8) / 16) as i32;
                if tick == 149 {
                    assert!(game.digs.contains_key(&id), "seed {seed}: 掘る道具を持って始まる");
                    assert_ne!(world.get(CellPos::new(7, row)).material, MaterialId::EMPTY);
                }
                if digging {
                    started.get_or_insert(tick);
                    let gone = world.get(CellPos::new(7, row)).material == MaterialId::EMPTY;
                    if gone && removed_at.is_none() {
                        removed_at = Some(tick);
                    }
                }
            }
            let elapsed = removed_at.expect("壁が消える") - started.unwrap();
            assert!(
                (99..=101).contains(&elapsed),
                "seed {seed}: 硬さ 600 / 力 6 = 100 tick ({elapsed})"
            );
            // 掘った石は持ち物に集まり (仕様 10 §13)、game.ron に写して読み戻せる。
            let id = player.unwrap();
            let stone = world.materials().id("stone").unwrap();
            let held = game.inventories[&id].count(stone);
            assert!(held > 15, "seed {seed}: 掘った石が集まる ({held})");
            let save = charon_game::save::GameSave::capture(
                &game,
                &entities,
                id,
                world.materials(),
                world.tick(),
            );
            assert!(
                save.player
                    .as_ref()
                    .unwrap()
                    .inventory
                    .iter()
                    .any(|(n, c)| n == "stone" && *c == held)
            );
            save.write(&dir.0).unwrap();
            assert_eq!(charon_game::save::GameSave::read(&dir.0).unwrap(), Some(save));
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
