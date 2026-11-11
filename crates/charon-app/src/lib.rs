//! 仕様 06 §6〜§7: 固定 tick と、ウィンドウ・サーフェスの復帰管理。

#[cfg(target_arch = "wasm32")]
mod browser_storage;
mod gesture;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod headless_tests;
mod pointer;
mod replay;
mod timestep;
mod ui;

pub use pointer::{PointerEvent, PointerInput};
pub use replay::{BrushOp, Replay};
pub use timestep::FixedTimestep;
use timestep::advance_frame;

#[cfg(target_arch = "wasm32")]
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use charon_core::cell::MaterialId;
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::world::World;
use charon_game::EntityWorld;
use charon_physics::{CELLS_PER_METER, PhysicsEngine};
use charon_render::{
    BackgroundLayerPass, BackgroundPass, BackgroundSource, CellPass, CompositePass, EntityPass,
    EntitySprite, FullscreenPass, GpuContext, LightPass, PassList,
};
use charon_world::background::BackgroundWorld;
use charon_world::building::{BuildOp, Layer, apply_build};
#[cfg(target_arch = "wasm32")]
use charon_world::generator::ChunkGenerator;
use charon_world::save::WorldMeta;
#[cfg(target_arch = "wasm32")]
use charon_world::streaming::MemoryStore;
use charon_world::streaming::Streamer;
#[cfg(target_arch = "wasm32")]
use charon_world::web_save::WebSnapshot;
use gesture::TouchGestures;
use ui::Gui;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
#[cfg(target_arch = "wasm32")]
use winit::platform::web::{EventLoopExtWebSys, WindowExtWebSys};
use winit::window::{Window, WindowId};

/// 初期ワールドを受け取り、どのプラットフォームでも同じイベント処理を走らせる。
/// Android では呼び出し側が `android-native-activity` の EventLoop を作る。
pub fn run(
    event_loop: EventLoop<()>,
    world: World,
    brush_material: MaterialId,
    make_world: fn(u64) -> World,
) -> Result<(), winit::error::EventLoopError> {
    event_loop.run_app(&mut App::new(world, brush_material, make_world))
}

/// ブラウザー上で同じ描画・入力ループを使い、保存だけ Web アダプターへ渡す。
#[cfg(target_arch = "wasm32")]
pub fn run_web(
    event_loop: EventLoop<()>,
    world: World,
    brush_material: MaterialId,
    make_world: fn(u64) -> World,
    generator: Arc<dyn ChunkGenerator>,
    physics: PhysicsEngine,
) -> Result<(), String> {
    let materials = Arc::clone(world.materials());
    let mut app = App::new(world, brush_material, make_world);
    app.physics = physics;
    app.physics_initialized = false;
    let mut metadata = WorldMeta::new(app.world.seed(), &materials);
    let store = Arc::new(MemoryStore::new());
    match browser_storage::load() {
        Ok(Some(snapshot)) => {
            let restored = BackgroundWorld::from_bytes(
                &snapshot.background,
                &materials,
                &snapshot.metadata.materials,
            )
            .map_err(|err| err.to_string())
            .and_then(|background| {
                PhysicsEngine::decode_bodies(
                    &snapshot.bodies,
                    &materials,
                    &snapshot.metadata.materials,
                )
                .map(|physics| (background, physics))
                .map_err(|err| err.to_string())
            });
            match restored {
                Ok((background, physics)) => {
                    store.restore(snapshot.chunks).map_err(|err| err.to_string())?;
                    metadata = snapshot.metadata;
                    app.world = World::new(Arc::clone(&materials), metadata.seed);
                    app.world.restore_tick(metadata.tick);
                    app.background = background;
                    app.physics = physics;
                    app.web_status = format!("保存済み: tick {} を読み込みました", metadata.tick);
                }
                Err(err) => app.web_status = format!("未保存: 保存データを復元できません: {err}"),
            }
        }
        Ok(None) => {}
        Err(err) => app.web_status = format!("未保存: {err}"),
    }
    let streamer = Streamer::with_memory_store(
        Arc::clone(&store),
        materials,
        metadata.seed,
        generator,
        3,
        metadata.materials.clone(),
    )
    .map_err(|err| err.to_string())?;
    app.streaming = Some(StreamingState { streamer, metadata, store });
    event_loop.spawn_app(app);
    Ok(())
}

/// 無限ワールドと保存を接続してウィンドウを起動する。
/// 起動に必要なワールド・保存先・物理状態をそれぞれ明示的に渡す。
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
pub fn run_with_streaming(
    event_loop: EventLoop<()>,
    world: World,
    brush_material: MaterialId,
    make_world: fn(u64) -> World,
    streamer: Streamer,
    save_root: PathBuf,
    metadata: WorldMeta,
    physics: PhysicsEngine,
) -> Result<(), Box<dyn std::error::Error>> {
    let background = BackgroundWorld::load(&save_root, world.materials(), &metadata.materials)?;
    let mut app = App::new(world, brush_material, make_world);
    app.background = background;
    app.streaming = Some(StreamingState { streamer, save_root, metadata });
    app.physics = physics;
    app.physics_initialized = false;
    let result = event_loop.run_app(&mut app);
    if let Err(err) = app.save_world() {
        eprintln!("ワールドの保存に失敗: {err}");
    }
    result.map_err(Into::into)
}

struct StreamingState {
    streamer: Streamer,
    #[cfg(not(target_arch = "wasm32"))]
    save_root: PathBuf,
    metadata: WorldMeta,
    #[cfg(target_arch = "wasm32")]
    store: Arc<charon_world::streaming::MemoryStore>,
}

#[derive(Debug)]
struct BackgroundAdapter<'a>(&'a mut BackgroundWorld);

impl BackgroundSource for BackgroundAdapter<'_> {
    fn chunk(&self, pos: ChunkPos) -> Option<&[MaterialId; charon_core::coords::CHUNK_AREA]> {
        self.0.chunk(pos).map(|chunk| chunk.cells.as_ref())
    }

    fn take_render_dirty(&mut self, pos: ChunkPos) -> Option<charon_core::chunk::Rect> {
        self.0.take_render_dirty(pos)
    }
}

#[derive(Clone, Copy)]
struct Grab {
    body: usize,
    id: u64,
    offset: [f32; 2],
}

struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuContext>,
    #[cfg(target_arch = "wasm32")]
    gpu_pending: Rc<RefCell<Option<Result<GpuContext, String>>>>,
    #[cfg(target_arch = "wasm32")]
    gpu_generation: Rc<Cell<u64>>,
    #[cfg(target_arch = "wasm32")]
    web_status: String,
    passes: Option<PassList>,
    gui: Option<Gui>,
    world: World,
    background: BackgroundWorld,
    entities: EntityWorld,
    physics: PhysicsEngine,
    physics_initialized: bool,
    grab: Option<Grab>,
    streaming: Option<StreamingState>,
    #[cfg(not(target_arch = "wasm32"))]
    make_world: fn(u64) -> World,
    operations: Vec<BrushOp>,
    brush_material: MaterialId,
    pointer: PointerInput,
    gestures: TouchGestures,
    timer: FixedTimestep,
    previous: Option<Instant>,
    occluded: bool,
}

impl App {
    fn new(world: World, brush_material: MaterialId, make_world: fn(u64) -> World) -> Self {
        #[cfg(target_arch = "wasm32")]
        let _ = make_world;
        Self {
            window: None,
            gpu: None,
            #[cfg(target_arch = "wasm32")]
            gpu_pending: Rc::new(RefCell::new(None)),
            #[cfg(target_arch = "wasm32")]
            gpu_generation: Rc::new(Cell::new(0)),
            #[cfg(target_arch = "wasm32")]
            web_status: "未保存".to_owned(),
            passes: None,
            gui: None,
            world,
            background: BackgroundWorld::new(),
            entities: EntityWorld::new(),
            physics: PhysicsEngine::new(),
            physics_initialized: true,
            grab: None,
            streaming: None,
            #[cfg(not(target_arch = "wasm32"))]
            make_world,
            operations: Vec::new(),
            brush_material,
            pointer: PointerInput::default(),
            gestures: TouchGestures::default(),
            timer: FixedTimestep::default(),
            previous: None,
            occluded: false,
        }
    }

    fn release_grab(&mut self) {
        if let Some(grab) = self.grab.take() {
            self.physics.end_grab(grab.body);
        }
    }

    /// true なら剛体がポインターを使ったため、ブラシは置かない。
    fn handle_grab(&mut self, event: PointerEvent, zoom: u32, origin: [f32; 2]) -> bool {
        let (id, pointer) = match event {
            PointerEvent::Down { id, pos } | PointerEvent::Move { id, pos } => (id, Some(pos)),
            PointerEvent::Up { id, .. } => (id, None),
        };
        if let Some(grab) = self.grab {
            if grab.id == id {
                if let Some(pos) = pointer {
                    if pos[0].is_finite() && pos[1].is_finite() {
                        let x = origin[0] + pos[0] / zoom.max(1) as f32;
                        let y = origin[1] + pos[1] / zoom.max(1) as f32;
                        self.physics.drag_body(grab.body, [x + grab.offset[0], y + grab.offset[1]]);
                    }
                } else {
                    self.release_grab();
                }
                return true;
            }
            return false;
        }
        let PointerEvent::Down { pos, .. } = event else { return false };
        if !self.physics_initialized || !pos[0].is_finite() || !pos[1].is_finite() {
            return false;
        }
        let x = origin[0] + pos[0] / zoom.max(1) as f32;
        let y = origin[1] + pos[1] / zoom.max(1) as f32;
        let cell = CellPos::new(x.floor() as i32, y.floor() as i32);
        let Some(index) = self.physics.bodies.iter().position(|body| body.stamped.contains(&cell))
        else {
            return false;
        };
        let Some(rigid) = self.physics.world.bodies.get(self.physics.bodies[index].handle) else {
            return false;
        };
        let center = rigid.translation() * CELLS_PER_METER;
        self.grab = Some(Grab { body: index, id, offset: [center.x - x, center.y - y] });
        self.physics.begin_grab(index);
        true
    }

    fn init_passes(&mut self, size: winit::dpi::PhysicalSize<u32>) -> Result<(), String> {
        let Some(gpu) = self.gpu.as_ref() else { return Ok(()) };
        if size.width == 0 || size.height == 0 || gpu.surface_config().is_none() {
            return Ok(());
        }
        if let Some(passes) = self.passes.as_mut() {
            passes.resize(gpu, size.width, size.height);
            return Ok(());
        }
        let mut passes = PassList::new(gpu, size.width, size.height, 4);
        let layer_count = (size.width.div_ceil(64) + 1) * (size.height.div_ceil(64) + 1);
        let cells =
            CellPass::new(gpu, self.world.materials(), PassList::scene_format(), layer_count)
                .map_err(|err| err.to_string())?;
        passes.add(BackgroundPass::new(wgpu::Color { r: 0.08, g: 0.10, b: 0.14, a: 1.0 }));
        passes.add(
            BackgroundLayerPass::new(gpu, self.world.materials(), layer_count)
                .map_err(|err| err.to_string())?,
        );
        passes.add(cells);
        passes.add(EntityPass::new(gpu));
        passes.add(LightPass::new(gpu, passes.scene_size(), passes.light_format()));
        passes.add(CompositePass::new(gpu));
        passes.add(FullscreenPass::post_process(
            gpu,
            include_str!("../../charon-render/src/bloom.wgsl"),
        ));
        self.passes = Some(passes);
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn save_world(&mut self) -> Result<(), String> {
        let Some(mut state) = self.streaming.take() else { return Ok(()) };
        // 剛体は次の保存タスクで別ファイルに記録する。スタンプを地形として残さない。
        self.physics.unstamp_all(&mut self.world);
        state.streamer.save_loaded(&self.world).map_err(|err| err.to_string())?;
        state.streamer.finish().map_err(|err| err.to_string())?;
        self.background
            .save(&state.save_root, self.world.materials(), &state.metadata.materials)
            .map_err(|err| err.to_string())?;
        self.physics
            .save_bodies(&state.save_root, self.world.materials(), &state.metadata.materials)
            .map_err(|err| err.to_string())?;
        state.metadata.tick = self.world.tick();
        state.metadata.save(&state.save_root).map_err(|err| err.to_string())
    }

    #[cfg(target_arch = "wasm32")]
    fn save_browser(&mut self) -> Result<(), String> {
        let state = self.streaming.as_mut().ok_or("Web の保存領域がありません")?;
        let initialized = self.physics_initialized;
        if initialized {
            self.physics.unstamp_all(&mut self.world);
        }
        let result = (|| {
            state.streamer.save_loaded(&self.world).map_err(|err| err.to_string())?;
            state.streamer.flush().map_err(|err| err.to_string())?;
            state.metadata.tick = self.world.tick();
            let snapshot = WebSnapshot {
                metadata: state.metadata.clone(),
                chunks: state.store.snapshot().map_err(|err| err.to_string())?,
                background: self
                    .background
                    .to_bytes(self.world.materials(), &state.metadata.materials)
                    .map_err(|err| err.to_string())?,
                bodies: self
                    .physics
                    .encode_bodies(self.world.materials(), &state.metadata.materials)
                    .map_err(|err| err.to_string())?,
            };
            browser_storage::save(&snapshot)
        })();
        if initialized {
            self.physics.stamp_all(&mut self.world);
        }
        result
    }

    #[cfg(target_arch = "wasm32")]
    fn update_web_save_status(&mut self, result: Result<(), String>) {
        self.web_status = match result {
            Ok(()) => format!("保存済み: tick {}", self.world.tick()),
            Err(err) => format!("未保存: {err}"),
        };
        if let Some(gui) = self.gui.as_mut() {
            gui.controls.web_status.clone_from(&self.web_status);
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn process_actions(&mut self, _size: winit::dpi::PhysicalSize<u32>) {
        let save = self.gui.as_mut().is_some_and(|gui| std::mem::take(&mut gui.controls.web_save));
        if save {
            let result = self.save_browser();
            self.update_web_save_status(result);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn process_actions(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        const REPLAY_FILE: &str = "charon-sandbox-replay.ron";
        let Some(gui) = self.gui.as_mut() else { return };
        let save = std::mem::take(&mut gui.controls.save);
        let load = std::mem::take(&mut gui.controls.replay);
        if save {
            let replay = Replay {
                seed: self.world.seed(),
                tick: self.world.tick(),
                state_hash: self.world.state_hash(),
                operations: self.operations.clone(),
            };
            match ron::ser::to_string(&replay) {
                Ok(data) => {
                    if let Err(err) = std::fs::write(REPLAY_FILE, data) {
                        eprintln!("再生記録の保存に失敗: {err}");
                    }
                }
                Err(err) => eprintln!("再生記録の符号化に失敗: {err}"),
            }
        }
        if load {
            let result = std::fs::read_to_string(REPLAY_FILE)
                .map_err(|err| err.to_string())
                .and_then(|data| ron::from_str::<Replay>(&data).map_err(|err| err.to_string()))
                .and_then(|replay| {
                    let world = replay.play(self.make_world)?;
                    Ok((world, replay.operations))
                });
            match result {
                Ok((world, operations)) => {
                    self.world = world;
                    self.background = BackgroundWorld::new();
                    self.entities = EntityWorld::new();
                    self.physics = PhysicsEngine::new();
                    self.physics_initialized = true;
                    self.grab = None;
                    self.operations = operations;
                    self.timer.reset();
                    self.previous = None;
                    self.passes = None;
                    if let Err(err) = self.init_passes(size) {
                        eprintln!("再生後の描画パスの作成に失敗: {err}");
                    }
                }
                Err(err) => eprintln!("再生に失敗: {err}"),
            }
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(gpu), Some(passes)) =
            (self.window.as_ref(), self.gpu.as_ref(), self.passes.as_mut())
        else {
            return;
        };
        if self.occluded || gpu.surface_config().is_none() || window.inner_size().width == 0 {
            return;
        }
        let now = Instant::now();
        let elapsed = self.previous.replace(now).map_or_default(|previous| now - previous);
        let (zoom, origin, paused, step_once, light_quality) = self.gui.as_ref().map_or(
            (4, [0.0, 0.0], false, false, charon_render::LightQuality::High),
            |gui| {
                let controls = &gui.controls;
                (
                    controls.zoom,
                    controls.origin,
                    controls.paused,
                    controls.step_once,
                    controls.light_quality,
                )
            },
        );
        let (camera, radius) = camera_range(window.inner_size(), zoom, origin);
        if let Some(state) = self.streaming.as_mut() {
            state.streamer.set_load_radius(radius);
        }
        let world = &mut self.world;
        let streaming = &mut self.streaming;
        let physics = &mut self.physics;
        let entities = &mut self.entities;
        let initialized = &mut self.physics_initialized;
        let mut stream_error: Option<String> = None;
        let step = || {
            if stream_error.is_some() {
                return;
            }
            if let Some(state) = streaming.as_mut()
                && let Err(err) = state.streamer.tick(world, &[camera])
            {
                stream_error = Some(err.to_string());
                return;
            }
            if !*initialized {
                let ready = physics.bodies.iter().all(|body| {
                    physics.world.bodies.get(body.handle).is_some_and(|rigid| {
                        let position = rigid.translation() * CELLS_PER_METER;
                        world
                            .chunk(CellPos::new(position.x as i32, position.y as i32).chunk())
                            .is_some()
                    })
                });
                if !ready {
                    // 保存状態の初回読み込み中も tick を進め、30 tick 後の挿入を待つ。
                    // 剛体スタンプは対応チャンクが来るまで作らない。
                    world.step();
                    return;
                }
                physics.stamp_all(world);
                *initialized = true;
            }
            if let Err(err) = entities.step_players(&[], world) {
                stream_error = Some(err.to_string());
                return;
            }
            if let Err(err) = physics.step(world) {
                stream_error = Some(err.to_string());
            }
        };
        advance_frame(&mut self.timer, elapsed, paused, step_once, step);
        if paused
            && step_once
            && let Some(gui) = self.gui.as_mut()
        {
            gui.controls.step_once = false;
        }
        if let Some(err) = stream_error {
            eprintln!("ワールドの読み書きに失敗: {err}");
            event_loop.exit();
            return;
        }
        let tick_ms = now.elapsed().as_secs_f32() * 1000.0;
        passes.set_zoom(gpu, zoom);
        passes.set_light_quality(light_quality);
        let Some(surface) = gpu.surface() else { return };
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                let size = window.inner_size();
                if let Err(err) =
                    self.gpu.as_mut().map_or(Ok(()), |gpu| gpu.resize(size.width, size.height))
                {
                    eprintln!("サーフェスの復旧に失敗: {err}");
                    event_loop.exit();
                }
                return;
            }
            error => {
                eprintln!("フレーム取得に失敗: {error:?}");
                event_loop.exit();
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let scene_width = window.inner_size().width.div_ceil(zoom.max(1));
        let scene_height = window.inner_size().height.div_ceil(zoom.max(1));
        let visible =
            visible_chunks(&self.world, &self.background, origin, [scene_width, scene_height]);
        let mut background = BackgroundAdapter(&mut self.background);
        let sprites: Vec<_> = self
            .entities
            .iter()
            .map(|entity| EntitySprite {
                x16: entity.x16,
                y16: entity.y16,
                width: entity.width,
                height: entity.height,
            })
            .collect();
        if let Err(err) = passes.render_with_game(
            gpu,
            &mut self.world,
            Some(&mut background),
            &sprites,
            &visible,
            &view,
            origin,
        ) {
            eprintln!("描画に失敗: {err}");
            event_loop.exit();
            return;
        }
        if let Some(gui) = &mut self.gui {
            gui.render(gpu, window, &self.world, &view, tick_ms);
        }
        gpu.queue.present(frame);
        self.process_actions(window.inner_size());
    }
}

/// 前景が退避されても、独立した背景チャンクは可視領域で描く。
fn visible_chunks(
    world: &World,
    background: &BackgroundWorld,
    origin: [f32; 2],
    size: [u32; 2],
) -> Vec<ChunkPos> {
    let positions: BTreeSet<_> =
        world.chunk_positions().into_iter().chain(background.iter().map(|(&pos, _)| pos)).collect();
    positions
        .into_iter()
        .filter(|pos| {
            let corner = pos.origin();
            (corner.x as f32) < origin[0] + size[0] as f32
                && (corner.x + 64) as f32 > origin[0]
                && (corner.y as f32) < origin[1] + size[1] as f32
                && (corner.y + 64) as f32 > origin[1]
        })
        .collect()
}

/// 画面中心のチャンクと、四隅まで含む読み込み半径。
fn camera_range(
    size: winit::dpi::PhysicalSize<u32>,
    zoom: u32,
    origin: [f32; 2],
) -> (ChunkPos, i32) {
    let zoom = zoom.max(1);
    let center = CellPos::new(
        (origin[0] + size.width as f32 / (2 * zoom) as f32).floor() as i32,
        (origin[1] + size.height as f32 / (2 * zoom) as f32).floor() as i32,
    );
    let radius = (size.width.max(size.height) / zoom).div_ceil(128) as i32 + 2;
    (center.chunk(), radius)
}

/// ポインターをセル座標に変換し、指定した材質をブラシで置く。
fn apply_pointer(
    world: &mut World,
    material: MaterialId,
    event: PointerEvent,
    zoom: u32,
    origin: [f32; 2],
    radius: u32,
) -> Vec<BrushOp> {
    let pos = match event {
        PointerEvent::Down { pos, .. } | PointerEvent::Move { pos, .. } => pos,
        PointerEvent::Up { .. } => return Vec::new(),
    };
    if !pos[0].is_finite() || !pos[1].is_finite() {
        return Vec::new();
    }
    let zoom = zoom.max(1) as f32;
    let center = CellPos::new(
        (origin[0] + pos[0] / zoom).floor() as i32,
        (origin[1] + pos[1] / zoom).floor() as i32,
    );
    let radius = radius.clamp(1, 24) as i32;
    let mut operations = Vec::new();
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let cell = CellPos::new(center.x.saturating_add(dx), center.y.saturating_add(dy));
            if world.get(cell).material == material || world.chunk(cell.chunk()).is_none() {
                continue;
            }
            let new_cell = world.make_cell(material, cell);
            world.set(cell, new_cell);
            operations.push(BrushOp {
                tick: world.tick(),
                x: cell.x,
                y: cell.y,
                material: material.0,
            });
        }
    }
    operations
}

fn apply_build_pointer(
    world: &mut World,
    background: &mut BackgroundWorld,
    layer: Layer,
    material: MaterialId,
    event: PointerEvent,
    zoom: u32,
    origin: [f32; 2],
) -> Result<(), charon_world::building::BuildError> {
    let pos = match event {
        PointerEvent::Down { pos, .. } | PointerEvent::Move { pos, .. } => pos,
        PointerEvent::Up { .. } => return Ok(()),
    };
    if !pos[0].is_finite() || !pos[1].is_finite() {
        return Ok(());
    }
    let zoom = zoom.max(1) as f32;
    let cell = CellPos::new(
        (origin[0] + pos[0] / zoom).floor() as i32,
        (origin[1] + pos[1] / zoom).floor() as i32,
    );
    apply_build(world, background, BuildOp { pos: cell, layer, material })?;
    Ok(())
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(target_arch = "wasm32")]
        self.gpu_generation.set(self.gpu_generation.get().wrapping_add(1));
        self.timer.reset();
        self.previous = None;
        self.pointer = PointerInput::default();
        self.gestures = TouchGestures::default();
        self.release_grab();
        self.occluded = false;
        #[cfg(not(target_arch = "wasm32"))]
        let initial_size = LogicalSize::new(1024, 768);
        #[cfg(target_arch = "wasm32")]
        let initial_size = {
            let viewport = web_sys::window();
            let width = viewport
                .as_ref()
                .and_then(|web| web.inner_width().ok())
                .and_then(|value| value.as_f64())
                .map_or(1024.0, |value| value.clamp(320.0, 1440.0));
            let height = viewport
                .as_ref()
                .and_then(|web| web.inner_height().ok())
                .and_then(|value| value.as_f64())
                .map_or(768.0, |value| value.clamp(300.0, 1000.0));
            LogicalSize::new(width - 32.0, height - 120.0)
        };
        let window = match event_loop.create_window(
            Window::default_attributes()
                .with_title("Charon — live world")
                .with_inner_size(initial_size),
        ) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                eprintln!("ウィンドウの生成に失敗: {err}");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        #[cfg(target_arch = "wasm32")]
        {
            let Some(host) = web_sys::window()
                .and_then(|web| web.document())
                .and_then(|document| document.get_element_by_id("charon-canvas"))
            else {
                eprintln!("Web のキャンバス置き場がありません");
                event_loop.exit();
                return;
            };
            let Some(canvas) = window.canvas() else {
                eprintln!("Web のキャンバスを取得できません");
                event_loop.exit();
                return;
            };
            if host.append_child(&canvas).is_err() {
                eprintln!("Web のキャンバスを配置できません");
                event_loop.exit();
                return;
            }
        }
        if let Some(gpu) = &mut self.gpu {
            if let Err(err) = gpu.recreate_surface(Arc::clone(&window), size.width, size.height) {
                eprintln!("サーフェスの復帰に失敗: {err}");
                event_loop.exit();
                return;
            }
        } else {
            let descriptor = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
                event_loop.owned_display_handle(),
            ));
            #[cfg(not(target_arch = "wasm32"))]
            match pollster::block_on(GpuContext::new(
                descriptor,
                Arc::clone(&window),
                size.width,
                size.height,
            )) {
                Ok(gpu) => self.gpu = Some(gpu),
                Err(err) => {
                    eprintln!("GPU の初期化に失敗: {err}");
                    event_loop.exit();
                    return;
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                let pending = Rc::clone(&self.gpu_pending);
                let generation = Rc::clone(&self.gpu_generation);
                let current = generation.get();
                let wake = Arc::clone(&window);
                wasm_bindgen_futures::spawn_local(async move {
                    let result =
                        GpuContext::new(descriptor, Arc::clone(&wake), size.width, size.height)
                            .await
                            .map_err(|err| err.to_string());
                    if generation.get() == current {
                        *pending.borrow_mut() = Some(result);
                        wake.request_redraw();
                    }
                });
                self.window = Some(window);
                return;
            }
        }
        if let Err(err) = self.init_passes(size) {
            eprintln!("描画パスの初期化に失敗: {err}");
            event_loop.exit();
            return;
        }
        if let Some(gpu) = &self.gpu {
            let previous = self.gui.take().map(|gui| gui.controls);
            let mut gui = Gui::new(gpu, &window, self.brush_material);
            if let Some(controls) = previous {
                gui.controls = controls;
            } else {
                #[cfg(target_arch = "wasm32")]
                {
                    gui.controls.web_status = self.web_status.clone();
                }
            }
            self.gui = Some(gui);
        }
        window.request_redraw();
        self.window = Some(window);
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        #[cfg(target_arch = "wasm32")]
        {
            self.gpu_generation.set(self.gpu_generation.get().wrapping_add(1));
            self.gpu_pending.borrow_mut().take();
            let result = self.save_browser();
            self.update_web_save_status(result);
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.suspend();
        }
        self.window = None;
        self.release_grab();
        self.previous = None;
        self.timer.reset();
        // ワールド・デバイス・キュー・テクスチャは次の resumed に残す。
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.as_ref().map(Arc::clone) else { return };
        if window.id() != id {
            return;
        }
        #[cfg(target_arch = "wasm32")]
        if self.gpu.is_none() && matches!(&event, WindowEvent::RedrawRequested) {
            let ready = self.gpu_pending.borrow_mut().take();
            if let Some(result) = ready {
                match result {
                    Ok(gpu) => self.gpu = Some(gpu),
                    Err(err) => {
                        eprintln!("GPU の初期化に失敗: {err}");
                        if let Some(status) = web_sys::window()
                            .and_then(|web| web.document())
                            .and_then(|document| document.get_element_by_id("status"))
                        {
                            status.set_text_content(Some(&format!("GPU の初期化に失敗: {err}")));
                        }
                        event_loop.exit();
                        return;
                    }
                }
                if let Err(err) = self.init_passes(window.inner_size()) {
                    eprintln!("Web の描画パスの作成に失敗: {err}");
                    event_loop.exit();
                    return;
                }
                if let Some(gpu) = &self.gpu {
                    let mut gui = Gui::new(gpu, &window, self.brush_material);
                    gui.controls.web_status = self.web_status.clone();
                    self.gui = Some(gui);
                }
                if let Some(status) = web_sys::window()
                    .and_then(|web| web.document())
                    .and_then(|document| document.get_element_by_id("status"))
                {
                    status.set_text_content(Some("実行中 — ワールドの保存は Sandbox パネルから"));
                }
            }
        }
        let consumed = self.gui.as_mut().is_some_and(|gui| gui.on_event(&window, &event));
        let gesture = if let (WindowEvent::Touch(touch), Some(gui)) = (&event, self.gui.as_mut()) {
            self.gestures.update(
                touch.id,
                touch.phase,
                [touch.location.x as f32, touch.location.y as f32],
                &mut gui.controls,
            )
        } else {
            false
        };
        if let Some(pointer) = self.pointer.normalize(&event) {
            let (selected, zoom, origin, radius, build_layer) =
                self.gui.as_ref().map_or((self.brush_material, 4, [0.0, 0.0], 1, None), |gui| {
                    (
                        gui.controls.selected,
                        gui.controls.zoom,
                        gui.controls.origin,
                        gui.controls.brush_radius,
                        gui.controls.build_layer,
                    )
                });
            // つかみ中は GUI がイベントを消費しても Up を届け、別の指でブラシを描かない。
            let grabbing = self.grab.is_some();
            let used_by_body = if grabbing || (!consumed && !gesture) {
                self.handle_grab(pointer, zoom, origin)
            } else {
                false
            };
            if !grabbing && !used_by_body && !consumed && !gesture {
                if let Some(layer) = build_layer {
                    if let Err(err) = apply_build_pointer(
                        &mut self.world,
                        &mut self.background,
                        layer,
                        selected,
                        pointer,
                        zoom,
                        origin,
                    ) {
                        eprintln!("建築に失敗: {err}");
                    }
                } else {
                    self.operations.extend(apply_pointer(
                        &mut self.world,
                        selected,
                        pointer,
                        zoom,
                        origin,
                        radius,
                    ));
                }
            }
            window.request_redraw();
        }
        match event {
            WindowEvent::MouseWheel { delta, .. } if !consumed => {
                if let Some(grab) = self.grab {
                    let turns = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / 40.0,
                    };
                    if turns.is_finite() {
                        self.physics.rotate_body(grab.body, turns * 0.12);
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut()
                    && let Err(err) = gpu.resize(size.width, size.height)
                {
                    eprintln!("サーフェスのリサイズに失敗: {err}");
                    event_loop.exit();
                    return;
                }
                if size.width == 0 || size.height == 0 {
                    self.timer.reset();
                    self.previous = None;
                } else if let Err(err) = self.init_passes(size) {
                    eprintln!("描画パスのリサイズに失敗: {err}");
                    event_loop.exit();
                    return;
                }
                window.request_redraw();
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                self.timer.reset();
                self.previous = None;
                if !occluded {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if !self.occluded
            && let Some(window) = &self.window
            && self.gpu.as_ref().and_then(GpuContext::surface_config).is_some()
        {
            window.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_core::testkit::{box_art, material_of, world_from_ascii};

    #[test]
    #[allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。
    fn grabbed_body_moves_and_releases() {
        use charon_core::cell::Cell;
        let mut world = world_from_ascii(&box_art(128, 96), 1);
        let wood = material_of(&world, 'W');
        let mut physics = PhysicsEngine::new();
        physics
            .create_body(
                4,
                4,
                vec![Cell { material: wood, ..Cell::default() }; 16],
                world.materials(),
                [48.0, 32.0],
            )
            .unwrap();
        physics.stamp_all(&mut world);
        let mut app = App::new(world, wood, |_| world_from_ascii(&box_art(128, 96), 1));
        app.physics = physics;
        let hit = app.physics.bodies[0].stamped[0];
        let start = [hit.x as f32, hit.y as f32];
        assert!(app.handle_grab(PointerEvent::Down { id: 0, pos: start }, 1, [0.0, 0.0]));
        assert!(app.handle_grab(
            PointerEvent::Move { id: 0, pos: [start[0] + 20.0, start[1]] },
            1,
            [0.0, 0.0]
        ));
        let rigid = &app.physics.world.bodies[app.physics.bodies[0].handle];
        assert_eq!(rigid.translation().x * CELLS_PER_METER, 68.0);
        assert_eq!(rigid.gravity_scale(), 0.0);
        assert!(app.handle_grab(PointerEvent::Up { id: 0, pos: start }, 1, [0.0, 0.0]));
        assert!(app.grab.is_none());
        assert_eq!(app.physics.world.bodies[app.physics.bodies[0].handle].gravity_scale(), 1.0);
        app.physics.step(&mut app.world).unwrap();
        assert!(!app.physics.bodies[0].stamped.is_empty());
    }

    #[test]
    fn background_only_chunk_remains_visible() {
        let world = world_from_ascii(&box_art(32, 32), 1);
        let mut background = BackgroundWorld::new();
        background.set(CellPos::new(65, 1), material_of(&world, 'W'));
        let visible = visible_chunks(&world, &background, [64.0, 0.0], [64, 64]);
        assert_eq!(visible, [ChunkPos::new(1, 0)]);
    }

    #[test]
    fn brush_places_selected_material() {
        let mut world = world_from_ascii(&box_art(32, 32), 1);
        let selected = material_of(&world, 's');
        let pos = CellPos::new(10, 12);
        apply_pointer(
            &mut world,
            selected,
            PointerEvent::Down { id: 0, pos: [40.0, 48.0] },
            4,
            [0.0, 0.0],
            1,
        );
        assert_eq!(world.get(pos).material, selected);
        let previous = world.get(pos);
        apply_pointer(
            &mut world,
            selected,
            PointerEvent::Move { id: 0, pos: [40.0, 48.0] },
            4,
            [0.0, 0.0],
            1,
        );
        assert_eq!(world.get(pos), previous);
        apply_pointer(
            &mut world,
            selected,
            PointerEvent::Up { id: 0, pos: [60.0, 60.0] },
            4,
            [0.0, 0.0],
            1,
        );
        assert_ne!(world.get(CellPos::new(15, 15)).material, selected);
    }
}
