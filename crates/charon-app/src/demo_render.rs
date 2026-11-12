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
use charon_render::{GpuContext, LightQuality, PassList};
use charon_world::background::BackgroundWorld;
use charon_world::demo::{
    Biome, DemoGenerator, DemoMaterials, PLAYER_SPAWN, biome, tunnel_center, yellow_text_placement,
};
use charon_world::generator::ChunkGenerator;

use crate::demo::{demo_bodies, demo_loadout};
use crate::demo_ui::{DemoUi, FrameStats, Overlay};
use crate::sim::Sim;
use crate::ui::Controls;
use crate::{BackgroundAdapter, PointerEvent, apply_pointer, build_passes, visible_chunks};

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
            loadout: Some((&pack.definitions, &loadout)),
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
    let mut passes: PassList =
        build_passes(gpu, &materials, WIDTH, HEIGHT, crate::DEMO_CLEAR, crate::DEMO_AMBIENT)
            .unwrap();
    passes.set_zoom(gpu, ZOOM);
    passes.set_light_quality(LightQuality::High);
    let chunks = scene.world.chunk_positions();
    scene.generator.populate_background(SEED, &chunks, &mut scene.background, &mut scene.populated);
    let origin = Scene::origin(center);
    let visible =
        visible_chunks(&scene.world, &scene.background, origin, [WIDTH / ZOOM, HEIGHT / ZOOM]);
    let sprites = crate::sim::sprites(&scene.entities, &scene.game);
    let output = target(gpu);
    let view = output.create_view(&Default::default());
    let mut background = BackgroundAdapter(&mut scene.background);
    passes
        .render_with_game(
            gpu,
            &mut scene.world,
            Some(&mut background),
            &sprites,
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
