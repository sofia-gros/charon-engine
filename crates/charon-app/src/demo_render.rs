//! 仕様 09 §7: 実 GPU でデモの場面をオフスクリーンに描き、画素を検査する。
//!
//! GPU がなければ何もせず戻る (CI の GPU なし環境)。`CHARON_REQUIRED_GPU_ADAPTER` を指定すると、
//! その名前を含む離散 GPU でなければ失敗する。`CHARON_DEMO_CAPTURE_DIR` を指定すると、各場面の
//! 画像を `<場面>_world.rgba` (UI なし) と `<場面>_ui.rgba` (UI あり) の RGBA8 で書き出す。
#![allow(clippy::unwrap_used)] // GPU があるときは、失敗をその場で検出する。

use std::collections::BTreeSet;
use std::path::PathBuf;

use charon_core::cell::{Cell, MaterialId};
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::world::World;
use charon_game::PlayerInput;
use charon_game::game::{GameInput, GameState};
use charon_game::mods::{ModDirectory, ModPack};
use charon_game::projectile::Payload;
use charon_game::{EntityId, EntityWorld};
use charon_physics::PhysicsEngine;
use charon_render::{DeepBackgroundParams, GpuContext, LightQuality, PassList};
use charon_world::background::BackgroundWorld;
use charon_world::demo::{
    Biome, DemoGenerator, DemoMaterials, PLAYER_SPAWN, biome, tunnel_center, yellow_text_placement,
};
use charon_world::generator::ChunkGenerator;

use crate::demo::{demo_bodies, demo_enemies, demo_loadout};
use crate::demo_ui::{DemoUi, FrameStats, Overlay};
use crate::sim::Sim;
use crate::ui::Controls;
use crate::{BackgroundAdapter, PointerEvent, apply_pointer, visible_chunks};

const WIDTH: u32 = 1600;
const HEIGHT: u32 = 900;
const ZOOM: u32 = 4;
const SEED: u64 = 1;

/// 場面ごとの世界。`generator` で周囲のチャンクを直接読み込む (ストリーミングの遅延は使わない)。
struct Scene {
    world: World,
    background: BackgroundWorld,
    entities: EntityWorld,
    physics: PhysicsEngine,
    generator: DemoGenerator,
    populated: BTreeSet<ChunkPos>,
    game: GameState,
    player: Option<EntityId>,
    /// 背景のさらに奥の虹色の塊 (仕様 09 §9)。`None` なら描かない。
    deep: Option<DeepBackgroundParams>,
    /// 塊を動かす時刻 (tick)。
    tick: u64,
    /// 滑らかな剛体のスプライトを渡さない (スタンプしたセルが描かれないことを確かめる)。
    skip_smooth: bool,
    /// プレイヤーを画像 (32x64) で描く拡大率 (1 画素あたりのセル数)。`None` なら描かない。
    character_scale: Option<f32>,
    /// 光の品質。
    light: LightQuality,
    /// 画面の仮想スティックとボタン。`Some` なら UI の重ね描きに出す。
    pad: Option<crate::touch_pad::TouchPad>,
}

impl Scene {
    fn new(pack: &ModPack) -> Self {
        let generator =
            DemoGenerator { materials: DemoMaterials::from_table(&pack.materials).unwrap() };
        Self {
            world: pack.new_world(SEED),
            background: BackgroundWorld::new(),
            entities: EntityWorld::new(),
            physics: PhysicsEngine::new(),
            generator,
            populated: BTreeSet::new(),
            game: GameState::from_definitions(&pack.definitions),
            player: None,
            deep: Some(crate::demo::demo_deep_background(SEED)),
            tick: 0,
            skip_smooth: false,
            character_scale: None,
            light: LightQuality::High,
            pad: None,
        }
    }

    /// `center` を囲む 15×11 チャンクを生成して読み込む。
    fn load_area(&mut self, center: CellPos) {
        let middle = center.chunk();
        for cy in -5..=5 {
            for cx in -7..=7 {
                let pos = ChunkPos::new(middle.x + cx, middle.y + cy);
                let chunk = self.generator.generate(SEED, pos, self.world.materials());
                self.world.insert_loaded_chunk(pos, chunk);
            }
        }
    }

    fn origin(center: CellPos) -> [f32; 2] {
        let scene = [(WIDTH / ZOOM) as i32, (HEIGHT / ZOOM) as i32];
        [(center.x - scene[0] / 2) as f32, (center.y - scene[1] / 2) as f32]
    }
}

fn pack() -> ModPack {
    ModPack::load(&[ModDirectory {
        path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods/fse_demo"),
        script: false,
    }])
    .unwrap()
}

/// 部屋: 剛体 3 つとプレイヤーが床に落ち着く。
fn room_scene(pack: &ModPack) -> (Scene, CellPos) {
    let mut scene = Scene::new(pack);
    let center = CellPos::new(100, 232);
    scene.load_area(center);
    scene.physics = demo_bodies(&scene.world).unwrap();
    let (mut initialized, mut player, mut spawn, mut streaming) =
        (false, None, Some(PLAYER_SPAWN), None);
    let mut physics_ms = 0.0;
    let loadout = demo_loadout();
    let mut enemy_spawns = demo_enemies();
    for _ in 0..150 {
        Sim {
            world: &mut scene.world,
            streaming: &mut streaming,
            physics: &mut scene.physics,
            initialized: &mut initialized,
            entities: &mut scene.entities,
            player: &mut player,
            spawn: &mut spawn,
            game: &mut scene.game,
            definitions: Some(&pack.definitions),
            loadout: Some(&loadout),
            enemy_spawns: &mut enemy_spawns,
        }
        .tick(center.chunk(), PlayerInput::default(), GameInput::default(), &mut physics_ms)
        .unwrap();
    }
    assert!(player.is_some(), "プレイヤーが部屋に生成される");
    scene.player = player;
    (scene, center)
}

/// 通路の上に空間を掘り、虹色のブラシで砂を注いで山を作る (本物のブラシの経路を使う)。
fn rainbow_scene(pack: &ModPack) -> (Scene, CellPos) {
    let mut scene = Scene::new(pack);
    let cx = 300;
    let floor = tunnel_center(SEED, cx) + 9;
    let center = CellPos::new(cx, floor - 28);
    scene.load_area(center);
    for y in floor - 70..=floor {
        for x in cx - 75..=cx + 75 {
            scene.world.set(CellPos::new(x, y), Cell::default());
        }
    }
    let origin = Scene::origin(center);
    let first = scene.generator.materials.rainbow_sand;
    for tick in 0..420 {
        // ポインターを左右に動かしながら注ぐ。色は各セルの x 座標で決まる。
        let sweep = (tick * 2) % 80;
        let sweep = if sweep < 40 { sweep } else { 80 - sweep };
        let cell = CellPos::new(cx - 20 + sweep, floor - 60);
        let pos =
            [(cell.x as f32 - origin[0]) * ZOOM as f32, (cell.y as f32 - origin[1]) * ZOOM as f32];
        apply_pointer(
            &mut scene.world,
            MaterialId::EMPTY,
            Some(first),
            PointerEvent::Down { id: 0, pos },
            ZOOM,
            origin,
            3,
        );
        scene.world.step();
    }
    (scene, center)
}

/// 通路の中心線 (横に長く続く) の、指定した x の場所。
fn tunnel_scene(pack: &ModPack, x: i32) -> (Scene, CellPos) {
    let mut scene = Scene::new(pack);
    let center = CellPos::new(x, tunnel_center(SEED, x) - 6);
    scene.load_area(center);
    for _ in 0..10 {
        scene.world.step();
    }
    (scene, center)
}

fn yellow_text_scene(pack: &ModPack) -> (Scene, CellPos) {
    let (x0, y0) = (-100..100)
        .find_map(|band| yellow_text_placement(SEED, band).map(|(x0, y0, _)| (x0, y0)))
        .unwrap();
    let mut scene = Scene::new(pack);
    let center = CellPos::new(x0 + 35, y0 + 3);
    scene.load_area(center);
    for _ in 0..10 {
        scene.world.step();
    }
    (scene, center)
}

fn biome_x(kind: Biome) -> i32 {
    let band = (1..200).find(|&band| biome(SEED, band * 384 + 192) == kind).unwrap();
    band * 384 + 192
}

// ---- 描画と読み出し ------------------------------------------------------

fn target(gpu: &GpuContext) -> wgpu::Texture {
    gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("charon demo test output"),
        size: wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn read_back(gpu: &GpuContext, texture: &wgpu::Texture) -> Vec<u8> {
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("charon demo readback"),
        size: u64::from(WIDTH * HEIGHT * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH * 4), // 6400 は 256 の倍数。
                rows_per_image: Some(HEIGHT),
            },
        },
        wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    buffer.slice(..).get_mapped_range().unwrap().to_vec()
}

/// 世界だけを描いた画像 (RGBA8)。
fn render_world(gpu: &GpuContext, scene: &mut Scene, center: CellPos) -> (wgpu::Texture, Vec<u8>) {
    let materials = std::sync::Arc::clone(scene.world.materials());
    let (mut passes, clock): (PassList, _) = crate::build_passes_with(
        gpu,
        &materials,
        WIDTH,
        HEIGHT,
        crate::DEMO_CLEAR,
        crate::DEMO_AMBIENT,
        scene.deep,
    )
    .unwrap();
    if let Some(clock) = &clock {
        clock.set_tick(scene.tick);
    }
    passes.set_zoom(gpu, ZOOM);
    passes.set_light_quality(scene.light);
    let chunks = scene.world.chunk_positions();
    scene.generator.populate_background(SEED, &chunks, &mut scene.background, &mut scene.populated);
    let origin = Scene::origin(center);
    let visible =
        visible_chunks(&scene.world, &scene.background, origin, [WIDTH / ZOOM, HEIGHT / ZOOM]);
    let sprites = crate::sim::sprites(&scene.entities, &scene.game, &[]);
    let output = target(gpu);
    let view = output.create_view(&Default::default());
    let mut background = BackgroundAdapter(&mut scene.background);
    let smooth = if scene.skip_smooth { Vec::new() } else { crate::smooth_sprites(&scene.physics) };
    let asset = crate::demo::demo_character_image();
    let character: Vec<charon_render::Sprite<'_>> = scene
        .character_scale
        .zip(scene.player.and_then(|id| scene.entities.get(id)))
        .map(|(scale, entity)| charon_render::Sprite {
            id: 1,
            width: asset.width,
            height: asset.height,
            rgba: &asset.rgba,
            position: [
                (entity.x16 as f32 + f32::from(entity.width) * 8.0) / 16.0,
                (entity.y16 as f32 + f32::from(entity.height) * 16.0) / 16.0,
            ],
            pivot: asset.pivot,
            scale,
            angle: 0.0,
            flip_x: false,
            filter: asset.filter,
            lit: true,
            frame: Some(asset.frame_for(crate::Pose::Idle, 0)),
        })
        .into_iter()
        .collect();
    passes
        .render_with_sprites(
            gpu,
            &mut scene.world,
            Some(&mut background),
            &sprites,
            &smooth,
            &character,
            &visible,
            &view,
            origin,
        )
        .unwrap();
    let pixels = read_back(gpu, &output);
    (output, pixels)
}

/// 参照画面の UI を世界の画像の上に重ねて描いた画像。計測グラフは 130 フレーム分の値で埋める。
fn render_ui(
    gpu: &GpuContext,
    scene: &Scene,
    center: CellPos,
    texture: &wgpu::Texture,
    debug: bool,
) -> Vec<u8> {
    let mut demo = DemoUi::new(scene.world.materials());
    demo.show_collision = debug;
    demo.show_bodies = debug;
    demo.show_chunks = debug;
    let mut controls = Controls::new(scene.generator.materials.rainbow_sand);
    controls.rainbow = Some(scene.generator.materials.rainbow_sand);
    controls.zoom = ZOOM;
    controls.origin = Scene::origin(center);
    let context = egui::Context::default();
    let mut renderer = egui_wgpu::Renderer::new(
        &gpu.device,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        Default::default(),
    );
    let overlay = Overlay {
        world: &scene.world,
        physics: &scene.physics,
        entities: &scene.entities,
        game: &scene.game,
        player: scene.player,
        pad: scene.pad.as_ref(),
    };
    let view = texture.create_view(&Default::default());
    for frame in 0..130u32 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(WIDTH as f32, HEIGHT as f32),
            )),
            time: Some(f64::from(frame) / 60.0),
            ..Default::default()
        };
        let wobble = (frame as f32 * 0.35).sin();
        let stats = FrameStats {
            frame_ms: 1.9 + 0.25 * wobble,
            tick_ms: 2.1 + 0.8 * (frame as f32 * 0.21).cos().abs(),
            phys_ms: 0.05 + 0.02 * wobble.abs(),
        };
        let output = context.run_pass(input, |context| {
            demo.draw(context, &mut controls, &overlay, stats, 150);
        });
        let last = frame == 129;
        let shapes = if last { output.shapes } else { Vec::new() };
        crate::ui::paint_egui(
            &mut renderer,
            &context,
            gpu,
            [WIDTH, HEIGHT],
            &view,
            shapes,
            output.textures_delta,
        );
    }
    read_back(gpu, texture)
}

// ---- 画素の検査 ----------------------------------------------------------

struct Stats {
    mean_luma: f32,
    bright: usize,
    sign_blue: usize,
    orange: usize,
    green: usize,
    red: usize,
    hues: usize,
}

/// 彩度が高く明るい画素の色相を 12 の区間 (30°) に分ける。
fn hue_bin(r: i32, g: i32, b: i32) -> Option<usize> {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    if max <= 90 || (max - min) * 100 <= max * 50 {
        return None;
    }
    let delta = (max - min) as f32;
    let hue = if max == r {
        ((g - b) as f32 / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) as f32 / delta + 2.0
    } else {
        (r - g) as f32 / delta + 4.0
    };
    Some(((hue * 2.0) as usize).min(11))
}

/// 矩形 (画素、終端は含まない) の中で、30 個以上の画素が現れた色相の区間の数。
fn hues_in(pixels: &[u8], x0: usize, y0: usize, x1: usize, y1: usize) -> usize {
    let mut bins = [0usize; 12];
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * WIDTH as usize + x) * 4;
            let (r, g, b) =
                (i32::from(pixels[i]), i32::from(pixels[i + 1]), i32::from(pixels[i + 2]));
            if let Some(bin) = hue_bin(r, g, b) {
                bins[bin] += 1;
            }
        }
    }
    bins.iter().filter(|&&count| count >= 30).count()
}

fn stats(pixels: &[u8]) -> Stats {
    let mut stats =
        Stats { mean_luma: 0.0, bright: 0, sign_blue: 0, orange: 0, green: 0, red: 0, hues: 0 };
    let mut total = 0.0_f64;
    let mut bins = [0usize; 12];
    for px in pixels.as_chunks::<4>().0 {
        let (r, g, b) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
        total += f64::from(r * 3 + g * 6 + b) / 10.0;
        if r.max(g).max(b) > 110 {
            stats.bright += 1;
        }
        if b > 100 && b > r + 30 && b > g - 10 && g > r {
            stats.sign_blue += 1;
        }
        if r > 200 && (90..200).contains(&g) && b < 90 {
            stats.orange += 1;
        }
        if g > r + 30 && g > b + 30 {
            stats.green += 1;
        }
        if r > g + 40 && r > b + 40 {
            stats.red += 1;
        }
        if let Some(bin) = hue_bin(r, g, b) {
            bins[bin] += 1;
        }
    }
    stats.mean_luma = (total / (pixels.len() / 4) as f64) as f32;
    stats.hues = bins.iter().filter(|&&count| count > 80).count();
    stats
}

fn capture(name: &str, kind: &str, pixels: &[u8]) {
    if let Some(dir) = std::env::var_os("CHARON_DEMO_CAPTURE_DIR") {
        let path = PathBuf::from(dir).join(format!("{name}_{kind}.rgba"));
        std::fs::write(path, pixels).unwrap();
    }
}

/// ヘッドレスの GPU を得る。GPU のない CI では `None` (他のテストが生成と UI を確かめる)。
/// `CHARON_REQUIRED_GPU_ADAPTER` があるときは、その離散 GPU でなければ失敗する。
fn test_gpu() -> Option<GpuContext> {
    let required = std::env::var("CHARON_REQUIRED_GPU_ADAPTER").ok();
    let gpu = match pollster::block_on(GpuContext::headless()) {
        Ok(gpu) => gpu,
        Err(err) => {
            assert!(required.is_none(), "指定した GPU を使えません: {err}");
            return None;
        }
    };
    let info = gpu.adapter.get_info();
    if let Some(name) = required.as_deref() {
        assert!(
            info.name.contains(name) && info.device_type == wgpu::DeviceType::DiscreteGpu,
            "離散 GPU {name:?} が必要ですが、選択されたアダプターは {info:?} です"
        );
    }
    println!("GPU adapter: {info:?}");
    Some(gpu)
}

/// 矩形 (画素、終端は含まない) の中で `matches` が真になる画素の数。
fn count_in(
    pixels: &[u8],
    (x0, y0, x1, y1): (usize, usize, usize, usize),
    matches: impl Fn(i32, i32, i32) -> bool,
) -> usize {
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * WIDTH as usize + x) * 4;
            if matches(i32::from(pixels[i]), i32::from(pixels[i + 1]), i32::from(pixels[i + 2])) {
                n += 1;
            }
        }
    }
    n
}

/// 仕様 10 §6.3: `Wand` パネル (体力・マナのバー) と、飛んでいる弾が描かれる。
#[test]
fn demo_renders_wand_panel() {
    let Some(gpu) = test_gpu() else { return };
    let pack = pack();
    let (mut scene, center) = room_scene(&pack);
    let id = scene.player.unwrap();
    // 体力を減らして、バーが途中までになるようにする。
    scene.game.vitals.get_mut(id).unwrap().damage(35);
    let (_, without_shots) = render_world(&gpu, &mut scene, center);

    let player = scene.entities.get(id).unwrap().clone();
    for dx in [12, 16, 20] {
        let x16 = player.x16 + i64::from(dx) * 16;
        scene
            .game
            .projectiles
            .spawn(&scene.world, x16, player.y16 + 48, 0, 0, 0, 100, 0, Payload::None)
            .unwrap();
    }
    let (texture, world_pixels) = render_world(&gpu, &mut scene, center);
    assert_ne!(world_pixels, without_shots, "飛んでいる弾が描かれる");

    let ui_pixels = render_ui(&gpu, &scene, center, &texture, true);
    capture("wand", "world", &world_pixels);
    capture("wand", "ui", &ui_pixels);
    let top = (600, 0, 1000, 130);
    let red = |r: i32, g: i32, b: i32| r > 170 && g < 90 && b < 90;
    let blue = |r: i32, g: i32, b: i32| b > 190 && r < 140 && g < 140;
    let (hp_ui, hp_world) = (count_in(&ui_pixels, top, red), count_in(&world_pixels, top, red));
    let (mana_ui, mana_world) =
        (count_in(&ui_pixels, top, blue), count_in(&world_pixels, top, blue));
    println!("wand panel: 赤 {hp_ui} (世界 {hp_world})、青 {mana_ui} (世界 {mana_world})");
    assert!(hp_ui > hp_world + 100, "体力のバーが描かれる ({hp_ui} / {hp_world})");
    assert!(mana_ui > mana_world + 100, "マナのバーが描かれる ({mana_ui} / {mana_world})");
}

/// 仕様 10 §9.5: ワイヤーが描かれ、`Wand` パネルに道具 (`Tool (right)`) の行が出る。
#[test]
fn demo_renders_hook() {
    let Some(gpu) = test_gpu() else { return };
    let pack = pack();
    let (mut scene, center) = room_scene(&pack);
    let id = scene.player.unwrap();
    assert!(scene.game.hooks.contains_key(&id), "デモのプレイヤーは右手にフックを持つ");
    let (_, without_rope) = render_world(&gpu, &mut scene, center);

    // 右ボタン相当の入力で、天井へワイヤーを撃つ。
    let seed = scene.world.seed();
    for tick in 0..8 {
        let input = GameInput { tool: true, aim: (16, -48), ..Default::default() };
        scene
            .game
            .step(
                &mut scene.world,
                &mut scene.physics,
                &mut scene.entities,
                &[(id, input)],
                tick,
                seed,
            )
            .unwrap();
    }
    let rope = scene.game.rope(id).expect("ワイヤーが出ている");
    assert!(rope.anchor().is_some(), "天井に着いている");
    let (texture, world_pixels) = render_world(&gpu, &mut scene, center);
    assert_ne!(world_pixels, without_rope, "ワイヤーが描かれる");
    let changed = world_pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(without_rope.as_chunks::<4>().0)
        .filter(|(a, b)| a != b)
        .count();
    println!("hook: ワイヤーで変わった画素 {changed}");
    assert!(changed > 200, "ワイヤーの点列が画素に現れる ({changed})");

    let ui_pixels = render_ui(&gpu, &scene, center, &texture, true);
    assert_ne!(ui_pixels, world_pixels);
    capture("hook", "world", &world_pixels);
    capture("hook", "ui", &ui_pixels);
}

/// 空の世界 (チャンクなし) の場面。画面の全体が「空」なので、奥の背景だけが見える。
/// xHigh: High と違う絵になり、光のあたる部分の色が濃く (彩度が高く) なる。画像は `CHARON_DEMO_CAPTURE_DIR` に出す。
#[test]
fn demo_renders_xhigh_light() {
    let Some(gpu) = test_gpu() else { return };
    let pack = pack();
    let (mut scene, center) = rainbow_scene(&pack);
    scene.light = LightQuality::High;
    let (_, high) = render_world(&gpu, &mut scene, center);
    scene.light = LightQuality::XHigh;
    let (_, xhigh) = render_world(&gpu, &mut scene, center);
    assert_ne!(high, xhigh, "xHigh は High と違う絵になる");
    let saturation = |pixels: &[u8]| -> f64 {
        let (mut sum, mut count) = (0.0, 0u32);
        for px in pixels.as_chunks::<4>().0 {
            let (max, min) = (px[..3].iter().max().unwrap(), px[..3].iter().min().unwrap());
            if *max > 60 {
                sum += f64::from(max - min) / f64::from(*max);
                count += 1;
            }
        }
        sum / f64::from(count.max(1))
    };
    let (sh, sx) = (saturation(&high), saturation(&xhigh));
    println!("light saturation: High {sh:.3}, xHigh {sx:.3}");
    assert!(sx > sh, "xHigh のほうが発色が濃い ({sh:.3} → {sx:.3})");
    capture("light_high", "world", &high);
    capture("light_xhigh", "world", &xhigh);
}

/// 仮想スティックとボタン (仕様 10 §10) が UI の重ね描きに出て、ボタンの位置の画素が変わる。
#[test]
fn demo_renders_touch_pad() {
    let Some(gpu) = test_gpu() else { return };
    let pack = pack();
    let (mut scene, center) = room_scene(&pack);
    let (texture, world_pixels) = render_world(&gpu, &mut scene, center);
    let without = render_ui(&gpu, &scene, center, &texture, false);
    let mut pad = crate::touch_pad::TouchPad::default();
    pad.set_viewport([WIDTH as f32, HEIGHT as f32]);
    pad.show();
    let layout = pad.layout();
    scene.pad = Some(pad);
    let (texture, _) = render_world(&gpu, &mut scene, center);
    let with = render_ui(&gpu, &scene, center, &texture, false);
    assert_ne!(with, without, "仮想パッドが描かれる");
    // 各ボタンの中心の近くは、パッドがあると明るくなる (半透明の白)。
    let luma = |pixels: &[u8], p: [f32; 2]| -> u32 {
        let (x, y) = (p[0] as usize, p[1] as usize);
        let i = (y * WIDTH as usize + x) * 4;
        u32::from(pixels[i]) + u32::from(pixels[i + 1]) + u32::from(pixels[i + 2])
    };
    for (name, p) in [("jump", layout.jump), ("hook", layout.hook), ("paint", layout.paint)] {
        let ring = [p[0] + layout.unit * 0.8, p[1]];
        assert!(luma(&with, ring) > luma(&without, ring), "{name} のボタンが描かれる");
    }
    capture("touch_pad", "ui", &with);
    let _ = world_pixels;
}

/// スプライトの拡大率 (仕様 06 §10): 同じ 32x64 の画像を、拡大率を 2 倍にすると面積が約 4 倍になる。
/// 実際の描画は CHARON_DEMO_CAPTURE_DIR に sprite_small / sprite_large で出す。
#[test]
fn demo_renders_sprite_with_scale() {
    let Some(gpu) = test_gpu() else { return };
    let pack = pack();
    let (mut scene, center) = room_scene(&pack);
    let (_, without) = render_world(&gpu, &mut scene, center);
    let base = 10.0 / 64.0;
    scene.character_scale = Some(base);
    let (_, small) = render_world(&gpu, &mut scene, center);
    scene.character_scale = Some(base * 2.0);
    let (_, large) = render_world(&gpu, &mut scene, center);
    let changed = |a: &[u8], b: &[u8]| -> usize {
        a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0).filter(|(x, y)| x != y).count()
    };
    let (small_area, large_area) = (changed(&small, &without), changed(&large, &without));
    println!("sprite area: small {small_area}, large {large_area}");
    assert!(small_area > 200, "画像が描かれる ({small_area})");
    let ratio = large_area as f64 / small_area as f64;
    assert!((3.0..5.0).contains(&ratio), "拡大率 2 倍で面積は約 4 倍 ({ratio:.2})");
    capture("sprite_small", "world", &small);
    capture("sprite_large", "world", &large);
}

fn empty_scene(pack: &ModPack) -> (Scene, CellPos) {
    (Scene::new(pack), CellPos::new(500, 300))
}

/// 彩度が高く明るい画素 (虹色の塊) の数。
fn vivid(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| {
            let (max, min) = (px[0].max(px[1]).max(px[2]), px[0].min(px[1]).min(px[2]));
            max > 90 && i32::from(max - min) * 100 > i32::from(max) * 50
        })
        .count()
}

/// 2 つの画像で、画素が (各色 `tolerance` 以内で) 同じ割合。比べる範囲は `(x0, y0, x1, y1)`、
/// `b` は `a` を `(dx, dy)` だけずらして比べる (`a[x + dx, y + dy]` と `b[x, y]`)。
fn same_fraction(
    a: &[u8],
    b: &[u8],
    (x0, y0, x1, y1): (usize, usize, usize, usize),
    (dx, dy): (usize, usize),
    tolerance: i32,
) -> f64 {
    let (mut same, mut total) = (0usize, 0usize);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * WIDTH as usize + x) * 4;
            let j = ((y + dy) * WIDTH as usize + x + dx) * 4;
            total += 1;
            if (0..3).all(|c| (i32::from(b[i + c]) - i32::from(a[j + c])).abs() <= tolerance) {
                same += 1;
            }
        }
    }
    same as f64 / total as f64
}

/// 仕様 09 §9: 背景のさらに奥で、虹色の塊がアニメーションしながら動き回る。
#[test]
fn demo_renders_deep_background() {
    let Some(gpu) = test_gpu() else { return };
    let pack = pack();

    // (1) 空の画面に、虹色の塊が描かれる。横方向に色が変わる (赤から紫まで)。
    let (mut scene, center) = empty_scene(&pack);
    scene.deep = None;
    let (_, plain) = render_world(&gpu, &mut scene, center);
    assert_eq!(vivid(&plain), 0, "塊を描かなければ、空の画面は暗いだけ");
    scene.deep = Some(crate::demo::demo_deep_background(SEED));
    let (_, frame0) = render_world(&gpu, &mut scene, center);
    let blobs = vivid(&frame0);
    let hues = hues_in(&frame0, 0, 0, WIDTH as usize, HEIGHT as usize);
    println!("deep: 虹色の画素 {blobs}、色相 {hues} 種類");
    assert!(blobs > 8_000, "虹色の塊が見える ({blobs})");
    assert!(hues >= 8, "赤から紫まで色が変わる ({hues} 種類)");
    capture("deep", "world", &frame0);

    // (2) 動き回る: 同じカメラでも、時間が進むと塊が動き、形もゆらぐ。
    scene.tick = 240;
    let (_, frame240) = render_world(&gpu, &mut scene, center);
    let moved = 1.0 - same_fraction(&frame0, &frame240, (0, 0, 1600, 900), (0, 0), 6);
    println!("deep: 240 tick で変わった画素の割合 {moved:.3}");
    assert!(moved > 0.04, "塊が動き回る ({moved:.3})");
    assert!(vivid(&frame240) > 8_000, "動いたあとも塊は見える");
    scene.tick = 241;
    let (_, frame241) = render_world(&gpu, &mut scene, center);
    let step = 1.0 - same_fraction(&frame240, &frame241, (0, 0, 1600, 900), (0, 0), 6);
    assert!(step < moved, "1 tick の動きは、240 tick の動きより小さい (滑らかに動く): {step:.4}");
    // 同じ時刻・同じカメラなら、同じ画像 (決定的)。
    scene.tick = 240;
    let (_, again) = render_world(&gpu, &mut scene, center);
    assert_eq!(again, frame240, "同じ入力で同じ画像");

    // (3) 視差: カメラが 40 セル動くと、塊は半分の 20 セル (80 画素) だけ動く。
    scene.tick = 0;
    let shifted = CellPos::new(center.x + 40, center.y);
    let (_, camera40) = render_world(&gpu, &mut scene, shifted);
    let parallax = same_fraction(&frame0, &camera40, (0, 100, 1400, 800), (80, 0), 6);
    println!("deep: 視差でずらして一致した画素の割合 {parallax:.4}");
    assert!(parallax > 0.97, "塊はカメラの動きの半分だけ動く ({parallax:.4})");
    let unshifted = same_fraction(&frame0, &camera40, (0, 100, 1400, 800), (0, 0), 6);
    // 塊は画面の 1 割ほどなので、空の画素は同じ。それでも、ずらして比べたときの 1.0 とは違う。
    assert!(
        unshifted < 0.99 && unshifted < parallax - 0.01,
        "ずらさずに比べると一致しない ({unshifted:.4})"
    );
}

/// 仕様 09 §9: 塊は光の影響を受けず、前景のセルの後ろに隠れる。
#[test]
fn deep_background_is_behind_cells() {
    let Some(gpu) = test_gpu() else { return };
    let pack = pack();
    let (mut scene, center) = empty_scene(&pack);
    // 画面の中央に、石の長方形を置く (40 × 24 セル)。
    let stone = scene.world.materials().id("stone").unwrap();
    let (x0, y0) = (center.x - 20, center.y - 12);
    for cy in y0.div_euclid(64)..=(y0 + 24).div_euclid(64) {
        for cx in x0.div_euclid(64)..=(x0 + 40).div_euclid(64) {
            scene.world.insert_chunk(ChunkPos::new(cx, cy));
        }
    }
    for y in y0..y0 + 24 {
        for x in x0..x0 + 40 {
            let pos = CellPos::new(x, y);
            let cell = scene.world.make_cell(stone, pos);
            scene.world.set(pos, cell);
        }
    }
    scene.deep = None;
    let (_, without) = render_world(&gpu, &mut scene, center);
    scene.deep = Some(crate::demo::demo_deep_background(SEED));
    let (_, with) = render_world(&gpu, &mut scene, center);

    // 石の内側 (縁から 8 画素はなす) は、塊の有無で画素がまったく変わらない。
    let origin = Scene::origin(center);
    let px = |cell: i32, o: f32| ((cell as f32 - o) * ZOOM as f32) as usize;
    let (left, top) = (px(x0, origin[0]) + 8, px(y0, origin[1]) + 8);
    let (right, bottom) = (px(x0 + 40, origin[0]) - 8, px(y0 + 24, origin[1]) - 8);
    let inside = same_fraction(&without, &with, (left, top, right, bottom), (0, 0), 0);
    assert_eq!(inside, 1.0, "前景のセルの後ろに隠れる (一致 {inside})");
    // 石の外では、塊が描かれて画像が変わる。
    let everything = same_fraction(&without, &with, (0, 0, 1600, 900), (0, 0), 0);
    assert!(everything < 0.985, "石の外には塊がある ({everything})");
    assert!(vivid(&with) > vivid(&without) + 5_000);
}

/// 画素の矩形 (セル座標 → 画素)。
fn cell_rect(center: CellPos, x0: i32, y0: i32, x1: i32, y1: i32) -> (usize, usize, usize, usize) {
    let origin = Scene::origin(center);
    let px = |cell: i32, o: f32| (((cell as f32 - o) * ZOOM as f32).max(0.0)) as usize;
    (
        px(x0, origin[0]),
        px(y0, origin[1]),
        px(x1, origin[0]).min(WIDTH as usize),
        px(y1, origin[1]).min(HEIGHT as usize),
    )
}

fn region_pixels(pixels: &[u8], (x0, y0, x1, y1): (usize, usize, usize, usize)) -> Vec<[u8; 4]> {
    let mut out = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * WIDTH as usize + x) * 4;
            out.push([pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]);
        }
    }
    out
}

/// 4×4 画素の塊の中の、明るさの最大と最小の差の平均。セルの格子 (ドット) だと小さく、
/// 縁が画素の細かさで滑らかに描かれていると大きい。
fn block_detail(pixels: &[u8], (x0, y0, x1, y1): (usize, usize, usize, usize)) -> f64 {
    let luma = |x: usize, y: usize| {
        let i = (y * WIDTH as usize + x) * 4;
        (i32::from(pixels[i]) * 3 + i32::from(pixels[i + 1]) * 6 + i32::from(pixels[i + 2])) / 10
    };
    let (mut total, mut blocks) = (0i64, 0i64);
    let block = ZOOM as usize;
    for by in (y0..y1 - block + 1).step_by(block) {
        for bx in (x0..x1 - block + 1).step_by(block) {
            let (mut lo, mut hi) = (i32::MAX, i32::MIN);
            for y in by..by + block {
                for x in bx..bx + block {
                    let l = luma(x, y);
                    lo = lo.min(l);
                    hi = hi.max(l);
                }
            }
            total += i64::from(hi - lo);
            blocks += 1;
        }
    }
    total as f64 / blocks as f64
}

/// 仕様 05 §11: 滑らかな剛体は、スタンプしたセルを描かず、画面の解像度で縁を滑らかに描く。
#[test]
fn demo_renders_smooth_body() {
    let Some(gpu) = test_gpu() else { return };
    let pack = pack();
    let (mut scene, center) = room_scene(&pack);
    assert_eq!(scene.physics.bodies[3].render, charon_physics::BodyRender::Smooth);
    // 滑らかな十字 (60, 226) とドットの十字 (80, 250) のまわり 28 × 28 セル。
    let smooth_area = cell_rect(center, 46, 212, 74, 240);
    let dots_area = cell_rect(center, 66, 236, 94, 264);

    // (1) 通常の描画。
    let (texture, with_sprite) = render_world(&gpu, &mut scene, center);
    // (2) 滑らかな剛体のスプライトを描かない: スタンプしたセルも描かれないので、何もない。
    scene.skip_smooth = true;
    let (_, hidden) = render_world(&gpu, &mut scene, center);
    // (3) すべての剛体を世界から外した画面。
    scene.physics.unstamp_all(&mut scene.world);
    let (_, bodiless) = render_world(&gpu, &mut scene, center);
    // スタンプを外すと光の遮りがわずかに変わるので、各色 6 以内の差を同じとみなす。
    let near = region_pixels(&hidden, smooth_area)
        .iter()
        .zip(region_pixels(&bodiless, smooth_area))
        .filter(|(a, b)| (0..3).all(|c| (i32::from(a[c]) - i32::from(b[c])).abs() <= 6))
        .count();
    let total = region_pixels(&hidden, smooth_area).len();
    println!("smooth: 描かれない領域で一致した画素 {near} / {total}");
    assert!(
        near as f64 > total as f64 * 0.995,
        "FLAG_SMOOTH のセルはセルの描画では描かれない ({near} / {total})"
    );
    let drawn = region_pixels(&with_sprite, smooth_area)
        .iter()
        .zip(region_pixels(&bodiless, smooth_area))
        .filter(|(a, b)| *a != b)
        .count();
    println!("smooth: 滑らかな十字で変わった画素 {drawn}");
    assert!(drawn > 1500, "滑らかな十字が描かれる ({drawn})");

    // (4) ドットの十字は、セルの描画で描かれる (従来どおり)。
    let dots_drawn = region_pixels(&hidden, dots_area)
        .iter()
        .zip(region_pixels(&bodiless, dots_area))
        .filter(|(a, b)| *a != b)
        .count();
    assert!(dots_drawn > 1500, "ドットの十字はセルとして描かれる ({dots_drawn})");

    // (5) 縁の細かさ: 滑らかな方が、画素の細かさで縁が変わる。
    let smooth_detail = block_detail(&with_sprite, smooth_area);
    let dots_detail = block_detail(&with_sprite, dots_area);
    println!("smooth: 縁の細かさ 滑らか {smooth_detail:.3} / ドット {dots_detail:.3}");
    assert!(
        smooth_detail > dots_detail * 1.25,
        "滑らかな縁は、セルの格子より細かい ({smooth_detail:.3} / {dots_detail:.3})"
    );
    capture("smooth", "world", &with_sprite);
    let ui = render_ui(&gpu, &scene, center, &texture, true);
    capture("smooth", "ui", &ui);
}

/// 仕様 05 §11: 滑らかな剛体は、角度を少し変えただけでも見た目が変わる (セルの格子にそろえない)。
#[test]
fn smooth_body_turns_below_cell_resolution() {
    let Some(gpu) = test_gpu() else { return };
    let pack = pack();
    let (mut scene, center) = room_scene(&pack);
    let area = cell_rect(center, 46, 212, 74, 240);
    let rotate = |scene: &mut Scene, delta: f32| scene.physics.rotate_body(3, delta);
    let (_, base) = render_world(&gpu, &mut scene, center);
    rotate(&mut scene, 0.02); // 約 1.1 度 (セル 1 つぶんの動きよりずっと小さい)
    let (_, turned) = render_world(&gpu, &mut scene, center);
    let changed = region_pixels(&base, area)
        .iter()
        .zip(region_pixels(&turned, area))
        .filter(|(a, b)| (0..3).any(|c| (i32::from(a[c]) - i32::from(b[c])).abs() > 3))
        .count();
    println!("smooth: 0.02 ラジアンで変わった画素 {changed}");
    assert!(changed > 60, "わずかな回転でも縁の画素が変わる ({changed})");
}

#[test]
fn demo_renders_on_gpu() {
    let Some(gpu) = test_gpu() else { return };
    let pack = pack();
    let scenes: Vec<(&str, (Scene, CellPos))> = vec![
        ("room", room_scene(&pack)),
        ("rainbow_pile", rainbow_scene(&pack)),
        ("yellow_biome", yellow_text_scene(&pack)),
        ("green_biome", tunnel_scene(&pack, biome_x(Biome::Green))),
        ("red_biome", tunnel_scene(&pack, biome_x(Biome::Red))),
    ];
    for (name, (mut scene, center)) in scenes {
        let (texture, world_pixels) = render_world(&gpu, &mut scene, center);
        let s = stats(&world_pixels);
        println!(
            "{name}: luma {:.1}, bright {}, blue {}, orange {}, green {}, red {}, hues {}",
            s.mean_luma, s.bright, s.sign_blue, s.orange, s.green, s.red, s.hues
        );
        capture(name, "world", &world_pixels);
        // どの場面も暗い洞窟で、明るい画素は発光する材質のまわりだけ。
        assert!(s.mean_luma < 110.0, "{name}: 平均の明るさ {:.1}", s.mean_luma);
        assert!(s.bright > 300, "{name}: 発光する材質が見える ({})", s.bright);
        match name {
            "room" => assert!(s.sign_blue > 150, "room: Tile Entity の看板 ({})", s.sign_blue),
            "rainbow_pile" => {
                // 山のある領域 (画面の中央) に、12 色のうち 11 色以上が縦縞として並ぶ。
                let pile = hues_in(&world_pixels, 640, 300, 980, 570);
                println!("rainbow_pile: 山の領域の色相 {pile} 種類");
                assert!(pile >= 11, "rainbow_pile: 山の色相 {pile} 種類");
            }
            "yellow_biome" => assert!(s.orange > 200, "yellow_biome: 橙の文字 ({})", s.orange),
            "green_biome" => assert!(s.green > 300, "green_biome: 緑の壁 ({})", s.green),
            "red_biome" => assert!(s.red > 300, "red_biome: 赤の壁 ({})", s.red),
            _ => {}
        }
        let ui_pixels = render_ui(&gpu, &scene, center, &texture, name == "room");
        assert_ne!(ui_pixels, world_pixels, "{name}: UI が重なっている");
        capture(name, "ui", &ui_pixels);
    }
}
