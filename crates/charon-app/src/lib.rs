//! 仕様 06 §6〜§7: 固定 tick と、ウィンドウ・サーフェスの復帰管理。

mod animation;
pub mod api;
pub mod assets;
pub mod audio;
#[cfg(target_arch = "wasm32")]
mod browser_storage;
pub mod camera;
#[cfg(not(target_arch = "wasm32"))]
pub mod demo;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod demo_render;
mod demo_ui;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
mod gamepad;
mod gesture;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod headless_tests;
mod player;
mod pointer;
mod replay;
pub mod script;
pub mod settings;
mod sim;
mod timestep;
mod touch_pad;
mod ui;

/// ゲームを作る人が使う名前をまとめたもの (仕様 14 §5)。`use charon_app::prelude::*;` で足りる。
/// ここに載せた名前は、互換性の方針 (`docs/API_POLICY.md`) の「安定」の対象。
pub mod prelude {
    pub use charon_core::cell::{Cell, MaterialId};
    pub use charon_core::coords::CellPos;
    pub use charon_core::material::MaterialTable;
    pub use charon_core::query::{RayHit, Stop};
    pub use charon_core::world::World;
    pub use charon_game::collapse::Collapse;
    pub use charon_game::defs::Definitions;
    pub use charon_game::drops::{Drops, Pickup};
    pub use charon_game::game::{GameEvent, GameInput, GameState};
    pub use charon_game::levitation::{Levitation, LevitationDef};
    pub use charon_game::status::{StatusKind, StatusTable};
    pub use charon_game::{EntityId, EntityWorld};

    pub use crate::animation::{Clip, Clips, SpriteAsset};
    pub use crate::api::{Context, Effects, Game, GameLogic};
    pub use crate::assets::{AssetError, Assets, SpriteSpec};
    pub use crate::audio::{SoundBank, SoundEvent};
    pub use crate::camera::CameraFx;
    pub use crate::script::ScriptGame;
    pub use crate::settings::{Action, KeyBindings, Settings};
    pub use crate::{AppOptions, EnemySpawn, PlayerLoadout};
}

pub use animation::{Clip, Clips, Pose, SpriteAsset};
pub use player::{KeyCommand, KeyState, follow_origin, try_spawn_player};
pub use pointer::{PointerEvent, PointerInput};
pub use replay::{BrushOp, Replay};
pub use settings::{Action, KeyBindings, Settings};
use sim::{Sim, sprites};
pub use timestep::FixedTimestep;
use timestep::advance_frame;
use touch_pad::{PadOutput, TouchPad};

#[cfg(target_arch = "wasm32")]
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;
use std::sync::Arc;
use web_time::Instant;

use charon_core::cell::MaterialId;
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::world::World;
use charon_game::defs::Definitions;
use charon_game::game::{GameInput, GameState};
#[cfg(not(target_arch = "wasm32"))]
use charon_game::save::GameSave;
use charon_game::wand::SpellTable;
use charon_game::{EntityId, EntityWorld};
use charon_physics::{BodyRender, CELLS_PER_METER, PhysicsEngine};
use charon_render::{
    BackgroundLayerPass, BackgroundPass, BackgroundSource, CellPass, CompositePass,
    DeepBackgroundClock, DeepBackgroundParams, DeepBackgroundPass, EntityPass, FullscreenPass,
    GpuContext, LightPass, PassList, SmoothBodyPass, SmoothBodySprite, SpritePass,
};
use charon_world::background::BackgroundWorld;
use charon_world::building::{BuildOp, Layer, apply_build};
use charon_world::demo::rainbow_material;
#[cfg(target_arch = "wasm32")]
use charon_world::generator::ChunkGenerator;
use charon_world::save::WorldMeta;
#[cfg(target_arch = "wasm32")]
use charon_world::streaming::MemoryStore;
use charon_world::streaming::Streamer;
#[cfg(target_arch = "wasm32")]
use charon_world::web_save::WebSnapshot;
use demo_ui::{DemoAction, FrameStats, Overlay, next_seed, paste, pointer_cell};
use gesture::TouchGestures;
use ui::Gui;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
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

/// 保存前のパネル表示。
#[cfg(any(target_arch = "wasm32", test))]
const NOT_SAVED: &str = "Not saved";

/// Sandbox パネルに出す保存状態。パネルの文字は egui の標準フォントで描くため、日本語は文字化けする。
/// そのため ASCII だけにする (ホストページの状態欄 `#status` は日本語でよい)。
#[cfg(any(target_arch = "wasm32", test))]
fn save_status_text(result: Result<(), String>, tick: u64) -> String {
    match result {
        Ok(()) => format!("Saved (tick {tick})"),
        Err(err) => format!("Not saved: {err}"),
    }
}

/// 保存データを読み込んで再開したときのパネル表示。
#[cfg(any(target_arch = "wasm32", test))]
fn load_status_text(tick: u64) -> String {
    format!("Loaded saved world (tick {tick})")
}

/// panic をブラウザーのコンソールと状態欄に出す。
///
/// フックがないと wasm の panic は `unreachable` というトラップにしかならず、原因が分からない上に、
/// 状態欄は起動時の「実行中」のまま残って、止まったことが見えない。
#[cfg(target_arch = "wasm32")]
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = format!("停止しました (panic): {info}");
        web_sys::console::error_1(&message.clone().into());
        set_web_status(&message);
    }));
}

/// ホストページの状態欄 (`#status`) を書き換える。
#[cfg(target_arch = "wasm32")]
fn set_web_status(text: &str) {
    if let Some(status) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("status"))
    {
        status.set_text_content(Some(text));
    }
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
    install_panic_hook();
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
                    app.web_status = load_status_text(metadata.tick);
                }
                Err(err) => app.web_status = format!("Not saved: cannot restore saved data: {err}"),
            }
        }
        Ok(None) => {}
        Err(err) => app.web_status = save_status_text(Err(err), 0),
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

/// 参照デモ向けの設定 (仕様 09)。既定値は従来のサンドボックスと同じ挙動。
#[derive(Clone, Debug, Default)]
pub struct AppOptions {
    /// カメラがプレイヤーを追い、キー入力でプレイヤーを動かす。`F` で自由カメラに切り替える。
    pub follow_player: bool,
    /// プレイヤーの初期位置 (左上のセル)。周囲のチャンクが読み込まれたら生成する。
    pub player_spawn: Option<[i32; 2]>,
    /// `rainbow_sand_0` の ID。あるとき、ブラシは x 座標で色を変える虹色が既定になる。
    pub rainbow_sand: Option<MaterialId>,
    /// 参照画面の UI を使う。
    pub demo: bool,
    /// 読み込んだチャンクの背景 (`green biome` の文字) を作る生成器。
    pub background: Option<charon_world::demo::DemoGenerator>,
    /// ゲームが読み込んだ呪文・杖・敵の定義 (仕様 10 §7)。エンジンの既定に MOD を重ねたもの。
    pub definitions: Option<Arc<Definitions>>,
    /// プレイヤーの初期装備。`definitions` とともにあるとき、プレイヤーは体力と杖を持ち、右ボタンで撃つ。
    pub player: Option<PlayerLoadout>,
    /// 背景のさらに奥で、視差で動く虹色の塊 (仕様 09 §9)。ゲームが渡す。`None` なら描かない。
    pub deep_background: Option<DeepBackgroundParams>,
    /// 世界に置く敵。周囲のチャンクが読み込まれたら `GameState::spawn_enemy` で生成する (仕様 10 §8.5)。
    pub enemies: Vec<EnemySpawn>,
    /// プレイヤーを描く画像 (仕様 06 §10)。`None` なら単色の箱。当たり判定の大きさは変わらない。
    pub player_sprite: Option<SpriteAsset>,
    /// 敵の種類の名前 (`EnemyDef::name`) ごとの画像 (仕様 06 §10.4)。ない種類は単色の箱。
    pub enemy_sprites: BTreeMap<String, SpriteAsset>,
    /// 設定 (`settings.ron`: UI の大きさ・キャラクターの大きさ・光・キー割り当て、仕様 10 §14) を読み書きする場所。`None` なら保存しない。
    pub settings_path: Option<std::path::PathBuf>,
    /// ゲームのロジック (仕様 14)。毎 tick 呼ばれる。
    pub logic: Option<api::GameLogic>,
    /// 効果音。`None` なら生成した音 (仕様 12、15)。
    pub sounds: Option<audio::SoundBank>,
}

/// ゲームがエンジンに渡す敵の生成の指定 (仕様 10 §8.5)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnemySpawn {
    /// `Definitions::enemy` に渡す敵の種類の名前。
    pub name: String,
    /// 左上のセル。
    pub cell: [i32; 2],
}

/// ゲームがエンジンに渡すプレイヤーの初期装備 (仕様 10 §7.4)。値も杖の名前もゲーム側が決める。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerLoadout {
    pub max_hp: i32,
    /// 左手の武器: `Definitions::wand` に渡す杖の名前。
    pub wand: String,
    /// 右手の道具: `Definitions::hook` に渡すワイヤーフックの名前 (仕様 10 §9.5)。
    pub hook: Option<String>,
    /// 右手の掘る道具: `Definitions::dig` に渡す名前 (仕様 10 §12)。
    pub dig: Option<String>,
    /// 浮遊 (ジャンプのボタンを押している間、燃料で上へ、仕様 10 §18)。
    pub levitation: Option<charon_game::levitation::LevitationDef>,
}

/// `Main Menu` の `Singleplayer` / `Random Seed` で作り直した、新しい世界の一式。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct DemoSession {
    pub world: World,
    pub streamer: Streamer,
    pub save_root: PathBuf,
    pub metadata: WorldMeta,
    pub physics: PhysicsEngine,
}

/// seed を受け取り、その seed の世界を作る。保存先は seed ごとに分けること。
#[cfg(not(target_arch = "wasm32"))]
pub type RestartFn = Box<dyn FnMut(u64) -> Result<DemoSession, Box<dyn std::error::Error>>>;

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
    run_with_streaming_options(
        event_loop,
        world,
        brush_material,
        make_world,
        streamer,
        save_root,
        metadata,
        physics,
        AppOptions::default(),
        None,
    )
}

/// [`run_with_streaming`] に、プレイヤーと参照デモの設定を足したもの。
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
pub fn run_with_streaming_options(
    event_loop: EventLoop<()>,
    world: World,
    brush_material: MaterialId,
    make_world: fn(u64) -> World,
    streamer: Streamer,
    save_root: PathBuf,
    metadata: WorldMeta,
    physics: PhysicsEngine,
    options: AppOptions,
    restart: Option<RestartFn>,
) -> Result<(), Box<dyn std::error::Error>> {
    let background = BackgroundWorld::load(&save_root, world.materials(), &metadata.materials)?;
    let mut app = App::new(world, brush_material, make_world);
    app.spawn_pending = options.player_spawn;
    app.enemies_pending = options.enemies.clone();
    app.options = options;
    app.reset_game();
    app.load_settings();
    if let Some(bank) = app.options.sounds.clone() {
        app.audio.set_bank(bank);
    }
    // 音声デバイスは起動時に 1 回だけ開く。開けなくても (デバイスなし等) 無音で続ける。
    if let Err(why) = app.audio.start_device() {
        eprintln!("音声を無効にします: {why}");
    }
    app.restart = restart;
    app.background = background;
    app.pending_game_save = read_game_save(&save_root);
    app.streaming = Some(StreamingState { streamer, save_root, metadata });
    app.physics = physics;
    app.physics_initialized = false;
    let result = event_loop.run_app(&mut app);
    app.save_settings();
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
    options: AppOptions,
    keys: KeyState,
    player: Option<EntityId>,
    /// 体力・杖・弾 (仕様 10 §6)。
    game: GameState,
    /// 背景の奥の塊を動かす時計 (`AppOptions::deep_background` があるとき)。
    deep_clock: Option<DeepBackgroundClock>,
    /// 左ボタン (武器) を押している間 true。
    weapon_down: bool,
    /// 右ボタン (道具) を押している間 true。
    tool_down: bool,
    /// 中ボタン (掘る道具) を押している間 true。
    dig_down: bool,
    /// 最後に見たカーソルの位置 (ウィンドウのピクセル)。
    cursor: [f32; 2],
    /// まだ生成していないプレイヤーの初期位置。生成に失敗したら `None` にする。
    spawn_pending: Option<[i32; 2]>,
    /// まだ生成していない敵。
    enemies_pending: Vec<EnemySpawn>,
    free_camera: bool,
    /// `Main Menu` で押された操作。描画の後に処理する。
    pending_action: Option<DemoAction>,
    restart_count: u64,
    ctrl: bool,
    selecting: bool,
    /// 背景を作り終えたチャンク (`AppOptions::background` があるとき)。
    bg_populated: BTreeSet<ChunkPos>,
    #[cfg(not(target_arch = "wasm32"))]
    restart: Option<RestartFn>,
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
    /// 画面の仮想スティックとボタン (仕様 10 §10)。
    touch_pad: TouchPad,
    /// デスクトップのゲームパッド (仕様 10 §11.2)。使えない環境では `None`。
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    gamepad: Option<gamepad::Gamepad>,
    /// 最後に歩いた向き (-1 か 1)。ゲームパッドのボタンで撃つ向きに使う。
    facing: i8,
    /// 画像のエンティティごとの向き (仕様 06 §10.4)。
    sprite_facing: BTreeMap<EntityId, i8>,
    /// 読み込んだ設定 (仕様 10 §14)。終了とメニューを閉じたときに、画面の値を写して書く。
    settings: Settings,
    /// 効果音 (仕様 12)。出来事から音を作って混ぜる。
    audio: audio::Audio,
    /// カメラの追従と揺れ (仕様 06 §11)。
    camera_fx: camera::CameraFx,
    /// `Game::on_start` を呼んだか。
    logic_started: bool,
    /// ゲームが頼んだ演出 (揺れ・音)。フレームの終わりに受け取る。
    effects: api::Effects,
    /// 読み込んだ `game.ron` (仕様 10 §13)。プレイヤーが生まれたら、体力・マナ・位置・持ち物を戻す。
    #[cfg(not(target_arch = "wasm32"))]
    pending_game_save: Option<GameSave>,
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
            web_status: NOT_SAVED.to_owned(),
            passes: None,
            gui: None,
            world,
            background: BackgroundWorld::new(),
            entities: EntityWorld::new(),
            options: AppOptions::default(),
            keys: KeyState::default(),
            player: None,
            game: GameState::new(SpellTable::default()),
            deep_clock: None,
            weapon_down: false,
            tool_down: false,
            dig_down: false,
            cursor: [0.0, 0.0],
            spawn_pending: None,
            enemies_pending: Vec::new(),
            free_camera: false,
            pending_action: None,
            restart_count: 0,
            ctrl: false,
            selecting: false,
            bg_populated: BTreeSet::new(),
            #[cfg(not(target_arch = "wasm32"))]
            restart: None,
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
            touch_pad: {
                let mut pad = TouchPad::default();
                // Android はタッチが主な操作なので、最初から出す。
                if cfg!(target_os = "android") {
                    pad.show();
                }
                pad
            },
            #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
            gamepad: gamepad::Gamepad::new(),
            facing: 1,
            sprite_facing: BTreeMap::new(),
            settings: Settings::default(),
            audio: audio::Audio::new(),
            camera_fx: camera::CameraFx::default(),
            logic_started: false,
            effects: api::Effects::default(),
            #[cfg(not(target_arch = "wasm32"))]
            pending_game_save: None,
            timer: FixedTimestep::default(),
            previous: None,
            occluded: false,
        }
    }

    /// `settings_path` の設定を読み、キー割り当てを入力に反映する。なければ既定。読めなければ報告して既定。
    // 呼び出し元 (起動) は native 専用。Web では保存しない。
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn load_settings(&mut self) {
        let Some(path) = self.options.settings_path.clone() else { return };
        match Settings::read(&path) {
            Ok(Some(settings)) => {
                self.keys.set_bindings(settings.keys.clone());
                self.settings = settings;
            }
            Ok(None) => {}
            Err(err) => eprintln!("{err}"),
        }
    }

    /// 画面の値 (UI の大きさ・キャラクターの大きさ・光・キー割り当て) を写して、`settings_path` に書く。
    fn save_settings(&mut self) {
        let Some(path) = self.options.settings_path.clone() else { return };
        if let Some(gui) = self.gui.as_ref() {
            let controls = &gui.controls;
            self.settings.ui_scale = controls.ui_scale;
            self.settings.character_scale = controls.sprite_scale;
            self.settings.light = settings::light_name(controls.light_quality).to_owned();
            self.settings.master_volume = controls.master_volume;
            self.settings.sfx_volume = controls.sfx_volume;
            self.settings.music_volume = controls.music_volume;
            self.settings.keys = controls.bindings.clone();
        }
        self.settings.sanitize();
        if let Err(err) = self.settings.write(&path) {
            eprintln!("{err}");
        }
    }

    /// 世界を作り直したときに、体力・杖・弾を初期状態へ戻す。
    // 呼び出し元 (リプレイ・世界の作り直し) は native 専用。Web では使わない。
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn reset_game(&mut self) {
        self.game = self
            .options
            .definitions
            .as_deref()
            .map_or_else(|| GameState::new(SpellTable::default()), GameState::from_definitions);
        // 掘る道具のあるゲームでは、支えを失った地形が落ちる (仕様 10 §16)。
        if self.options.player.as_ref().is_some_and(|loadout| loadout.dig.is_some()) {
            self.game.collapse = Some(charon_game::collapse::Collapse::new());
        }
        self.weapon_down = false;
        self.tool_down = false;
    }

    /// 左ボタンが武器になるか: プレイヤーがいて (装備を渡されていて)、描画モードでも自由カメラでもない。
    fn weapon_mode(&self) -> bool {
        self.options.follow_player
            && self.options.player.is_some()
            && !self.free_camera
            && !self.gui.as_ref().is_some_and(|gui| gui.controls.paint)
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
        let (clear, ambient) = if self.options.demo {
            (DEMO_CLEAR, DEMO_AMBIENT)
        } else {
            (DEFAULT_CLEAR, charon_render::DEFAULT_AMBIENT)
        };
        let (passes, clock) = build_passes_with(
            gpu,
            self.world.materials(),
            size.width,
            size.height,
            clear,
            ambient,
            self.options.deep_background,
        )?;
        self.passes = Some(passes);
        self.deep_clock = clock;
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
        // ゲーム状態 (プレイヤーの体力・マナ・位置・持ち物) は、世界とは別の game.ron (仕様 10 §13)。
        if let Some(id) = self.player {
            GameSave::capture(
                &self.game,
                &self.entities,
                id,
                self.world.materials(),
                self.world.tick(),
            )
            .write(&state.save_root)
            .map_err(|err| err.to_string())?;
        }
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
        self.web_status = save_status_text(result, self.world.tick());
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
                    self.reset_game();
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

    /// `Main Menu` の操作を、描画で借りたものを返した後に実行する。
    fn handle_demo_action(&mut self, event_loop: &ActiveEventLoop) {
        let Some(action) = self.pending_action.take() else { return };
        match action {
            DemoAction::Quit => event_loop.exit(),
            DemoAction::Singleplayer => self.restart_session(event_loop, 1),
            DemoAction::RandomSeed => {
                self.restart_count += 1;
                let seed = next_seed(self.world.seed(), self.restart_count);
                self.restart_session(event_loop, seed);
            }
        }
    }

    /// 今の世界を保存してから、`seed` の世界・保存・物理に入れ替える。作り直せなければ終了する。
    #[cfg(not(target_arch = "wasm32"))]
    fn restart_session(&mut self, event_loop: &ActiveEventLoop, seed: u64) {
        let Some(window) = self.window.clone() else { return };
        if self.restart.is_none() {
            eprintln!("この起動では世界を作り直せません");
            return;
        }
        self.save_settings();
        if let Err(err) = self.save_world() {
            eprintln!("ワールドの保存に失敗: {err}");
        }
        let session = match self.restart.as_mut().map(|restart| restart(seed)) {
            Some(Ok(session)) => session,
            Some(Err(err)) => {
                eprintln!("世界を作り直せません: {err}");
                event_loop.exit();
                return;
            }
            None => return,
        };
        let DemoSession { world, streamer, save_root, metadata, physics } = session;
        self.background = BackgroundWorld::load(&save_root, world.materials(), &metadata.materials)
            .unwrap_or_else(|err| {
                eprintln!("背景の読み込みに失敗: {err}");
                BackgroundWorld::new()
            });
        self.world = world;
        self.pending_game_save = read_game_save(&save_root);
        self.streaming = Some(StreamingState { streamer, save_root, metadata });
        self.physics = physics;
        self.physics_initialized = false;
        self.entities = EntityWorld::new();
        self.reset_game();
        self.player = None;
        self.spawn_pending = self.options.player_spawn;
        self.enemies_pending = self.options.enemies.clone();
        self.free_camera = false;
        self.bg_populated.clear();
        self.grab = None;
        self.operations.clear();
        self.timer.reset();
        self.previous = None;
        self.passes = None;
        if let Some(gui) = self.gui.as_mut() {
            gui.controls.origin = [0.0, 0.0];
            if let Some(demo) = gui.demo.as_mut() {
                demo.selection = None;
                demo.paste_armed = false;
            }
        }
        if let Err(err) = self.init_passes(window.inner_size()) {
            eprintln!("再開始後の描画パスの作成に失敗: {err}");
            event_loop.exit();
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn restart_session(&mut self, _event_loop: &ActiveEventLoop, _seed: u64) {}

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
        let (zoom, mut origin, paused, step_once, light_quality) = self.gui.as_ref().map_or(
            (4, [0.0, 0.0], false, false, charon_render::LightQuality::High),
            |gui| {
                let controls = &gui.controls;
                (
                    controls.zoom,
                    controls.origin,
                    controls.paused || controls.pause_menu,
                    controls.step_once,
                    controls.light_quality,
                )
            },
        );
        // 仕様 09 §4: 追従カメラ。自由カメラでは WASD / 矢印で動かし、プレイヤーは中立入力にする。
        let size = window.inner_size();
        if self.options.follow_player {
            if self.free_camera {
                let delta = self.keys.camera_delta();
                origin = [origin[0] + delta[0] as f32, origin[1] + delta[1] as f32];
            } else if let Some(player) = self.player.and_then(|id| self.entities.get(id)) {
                let target = follow_origin(player, [size.width, size.height], zoom);
                origin = self.camera_fx.step(target, elapsed.as_secs_f32(), true);
            } else {
                self.camera_fx.reset();
            }
            if let Some(gui) = self.gui.as_mut() {
                gui.controls.origin = origin;
            }
        }
        // キー割り当ては、メニューで変えられる。入力に反映する (仕様 10 §14)。
        if let Some(gui) = self.gui.as_ref()
            && gui.controls.bindings != *self.keys.bindings()
        {
            self.keys.set_bindings(gui.controls.bindings.clone());
        }
        // 画面のスティック・ボタン (仕様 10 §10)。キーボード・マウスの操作と、使っている方が優先される形で重ねる。
        self.touch_pad.set_viewport([size.width as f32, size.height as f32]);
        self.touch_pad.set_scale(self.gui.as_ref().map_or(1.0, |gui| gui.controls.ui_scale));
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        let pad_input = self.gamepad.as_mut().map(gamepad::Gamepad::poll).unwrap_or_default();
        #[cfg(any(target_arch = "wasm32", target_os = "android"))]
        let pad_input = PadOutput::default();
        let pad = if self.options.follow_player && !self.free_camera {
            self.touch_pad.output().merge(pad_input)
        } else {
            PadOutput::default()
        };
        if pad.toggle_paint {
            if let Some(gui) = self.gui.as_mut() {
                gui.controls.paint = !gui.controls.paint;
            }
            self.weapon_down = false;
        }
        let mut player_input = if self.free_camera {
            charon_game::PlayerInput::default()
        } else {
            self.keys.player_input()
        };
        if pad.horizontal != 0 {
            player_input.horizontal = pad.horizontal;
        }
        player_input.jump |= pad.jump;
        if player_input.horizontal != 0 {
            self.facing = player_input.horizontal;
        }
        // 仕様 10 §9.5: 左ボタン = 武器、右ボタン = 道具。どちらもカーソルの方向へ使う。
        let paint = self.gui.as_ref().is_some_and(|gui| gui.controls.paint);
        let mut action = if self.options.follow_player {
            let aim = self
                .player
                .and_then(|id| self.entities.get(id))
                .zip(pointer_cell(self.cursor, zoom, origin))
                .map(|(player, target)| player::aim_at(player, target));
            player::game_input(self.weapon_down, self.tool_down, paint, self.free_camera, aim)
        } else {
            GameInput::default()
        };
        if self.options.follow_player && !self.free_camera {
            // ゲームパッドのボタン: 向きは、狙いのスティックがあればそれ、なければ歩いている向きの少し上。
            let (button_fire, button_tool) = self.keys.pad_buttons();
            let (fire, tool) = (pad.fire || button_fire, pad.tool || button_tool);
            action.fire |= fire && !paint;
            action.tool |= tool;
            let dig = pad.dig || self.keys.dig_held() || self.dig_down;
            action.dig |= dig;
            if fire || tool || dig {
                action.aim = pad.aim.unwrap_or((i32::from(self.facing) * 1000, -300));
            }
        }
        let (camera, radius) = camera_range(size, zoom, origin);
        if let Some(state) = self.streaming.as_mut() {
            state.streamer.set_load_radius(radius);
        }
        let mut stream_error: Option<String> = None;
        let mut physics_ms = 0.0_f32;
        let mut sim = Sim {
            world: &mut self.world,
            streaming: &mut self.streaming,
            physics: &mut self.physics,
            initialized: &mut self.physics_initialized,
            entities: &mut self.entities,
            player: &mut self.player,
            spawn: &mut self.spawn_pending,
            game: &mut self.game,
            definitions: self.options.definitions.as_deref(),
            loadout: self.options.player.as_ref(),
            enemy_spawns: &mut self.enemies_pending,
        };
        let logic = self.options.logic.as_ref();
        let (logic_started, effects) = (&mut self.logic_started, &mut self.effects);
        let step = || {
            if stream_error.is_none() {
                let before = sim.game.event_count();
                if let Err(err) = sim.tick(camera, player_input, action, &mut physics_ms) {
                    stream_error = Some(err);
                } else if let Some(logic) = logic {
                    // ゲームのロジック (仕様 14): この tick に起きた出来事を渡して呼ぶ。
                    let events = sim.game.events_since(before).to_vec();
                    let tick = sim.world.tick();
                    let mut ctx = api::Context {
                        world: sim.world,
                        game: sim.game,
                        entities: sim.entities,
                        player: *sim.player,
                        tick,
                        effects,
                    };
                    logic.run(logic_started, &mut ctx, &events);
                }
            }
        };
        advance_frame(&mut self.timer, elapsed, paused, step_once, step);
        if paused
            && step_once
            && let Some(gui) = self.gui.as_mut()
        {
            gui.controls.step_once = false;
        }
        // 保存したゲーム状態を、プレイヤーが生まれた最初のフレームで戻す (仕様 10 §13)。
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(id) = self.player
            && let Some(save) = self.pending_game_save.take()
        {
            save.apply(&mut self.game, &mut self.entities, &self.world, id);
        }
        if let Some(err) = stream_error {
            eprintln!("ワールドの読み書きに失敗: {err}");
            event_loop.exit();
            return;
        }
        let tick_ms = now.elapsed().as_secs_f32() * 1000.0;
        let stats =
            FrameStats { frame_ms: elapsed.as_secs_f32() * 1000.0, tick_ms, phys_ms: physics_ms };
        // 背景の奥の塊を、世界の tick に合わせて動かす (仕様 09 §9)。
        if let Some(clock) = &self.deep_clock {
            clock.set_tick(self.world.tick());
        }
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
        if let Some(generator) = self.options.background {
            let chunks = self.world.chunk_positions();
            generator.populate_background(
                self.world.seed(),
                &chunks,
                &mut self.background,
                &mut self.bg_populated,
            );
        }
        let view = gpu.frame_view(&frame.texture);
        let scene_width = window.inner_size().width.div_ceil(zoom.max(1));
        let scene_height = window.inner_size().height.div_ceil(zoom.max(1));
        let visible =
            visible_chunks(&self.world, &self.background, origin, [scene_width, scene_height]);
        let mut background = BackgroundAdapter(&mut self.background);
        // 画像を割り当てたエンティティ (プレイヤー・敵) は、アニメーションする画像で描き、箱は描かない (仕様 06 §10)。
        let sprite_scale = self.gui.as_ref().map_or(1.0, |gui| gui.controls.sprite_scale);
        let drawn = animation::entity_sprites(
            self.player.zip(self.options.player_sprite.as_ref()),
            &self.options.enemy_sprites,
            &self.entities,
            &self.game.enemies,
            &mut self.sprite_facing,
            self.world.tick(),
            sprite_scale,
        );
        let boxes = sprites(&self.entities, &self.game, &drawn.skip);
        // 効果音 (仕様 12): 前のフレームとの状態の差から出来事を決めて、鳴らす。
        {
            let me = self.player.and_then(|id| self.entities.get(id).map(|entity| (id, entity)));
            let snapshot = audio::Snapshot {
                grounded: me.is_some_and(|(_, entity)| entity.grounded),
                vy: me.map_or(0, |(_, entity)| entity.vy),
                projectiles: self.game.projectiles.len(),
                anchored: me.is_some_and(|(id, _)| {
                    self.game.rope(id).is_some_and(|rope| rope.anchor().is_some())
                }),
                digging: me
                    .is_some_and(|(id, _)| self.game.dig_beam(id).is_some_and(|beam| beam.hit)),
                items: me
                    .and_then(|(id, _)| self.game.inventories.get(&id))
                    .map_or(0, charon_game::inventory::Inventory::total),
            };
            if let Some(gui) = self.gui.as_ref() {
                let controls = &gui.controls;
                self.audio.set_volumes(
                    controls.master_volume,
                    controls.sfx_volume,
                    controls.music_volume,
                );
            }
            for event in self.audio.update(snapshot) {
                // 画面の揺れ: 撃つ・フックが刺さる・掘り始める、で少しだけ。
                match event {
                    audio::SoundEvent::Shoot => self.camera_fx.shake(0.12),
                    audio::SoundEvent::HookAnchor => self.camera_fx.shake(0.2),
                    _ => {}
                }
            }
            // ゲームの出来事 (仕様 10 §15) は、ロジックが tick ごとに見たので、ここで捨てる。
            drop(self.game.take_events());
            // ゲームのロジックが頼んだ演出 (仕様 14)。
            let effects = std::mem::take(&mut self.effects);
            self.camera_fx.shake(effects.shake);
            self.audio.play_events(&effects.sounds);
            self.audio.pump();
        }
        let smooth = smooth_sprites(&self.physics);
        if let Err(err) = passes.render_with_sprites(
            gpu,
            &mut self.world,
            Some(&mut background),
            &boxes,
            &smooth,
            &drawn.sprites,
            &visible,
            &view,
            origin,
        ) {
            eprintln!("描画に失敗: {err}");
            event_loop.exit();
            return;
        }
        if let Some(gui) = &mut self.gui {
            let scene = Overlay {
                world: &self.world,
                physics: &self.physics,
                entities: &self.entities,
                game: &self.game,
                player: self.player,
                pad: (self.options.follow_player && self.touch_pad.visible())
                    .then_some(&self.touch_pad),
            };
            let action = gui.render(gpu, window, &scene, &view, stats);
            if action.is_some() {
                self.pending_action = action;
            }
        }
        gpu.queue.present(frame);
        self.process_actions(window.inner_size());
    }
}

/// `save_root/game.ron` を読む。なければ `None`、読めなければ報告して `None` (新しく始める)。
#[cfg(not(target_arch = "wasm32"))]
fn read_game_save(save_root: &std::path::Path) -> Option<GameSave> {
    match GameSave::read(save_root) {
        Ok(save) => save,
        Err(err) => {
            eprintln!("{err}");
            None
        }
    }
}

/// 従来のサンドボックスの背景色と、参照デモの (光の届かない空間がほぼ黒の) 背景色。
pub const DEFAULT_CLEAR: wgpu::Color = wgpu::Color { r: 0.08, g: 0.10, b: 0.14, a: 1.0 };
pub const DEMO_CLEAR: wgpu::Color = wgpu::Color { r: 0.012, g: 0.014, b: 0.02, a: 1.0 };
/// 参照デモの環境光。従来の `DEFAULT_AMBIENT` より暗く、洞窟は光る材質のまわりだけが見える。
pub const DEMO_AMBIENT: f32 = 0.045;

/// 背景・セル・背景レイヤー・エンティティ・光・合成・ブルームの描画パスを、この順で並べる (仕様 06 §4)。
/// アプリ本体と、実 GPU でのオフスクリーン描画の検査が同じ構成を使う。
/// `clear` は光の届かない空間の背景色、`ambient` はそこでの明るさの下限。
pub fn build_passes(
    gpu: &GpuContext,
    materials: &charon_core::material::MaterialTable,
    width: u32,
    height: u32,
    clear: wgpu::Color,
    ambient: f32,
) -> Result<PassList, String> {
    build_passes_with(gpu, materials, width, height, clear, ambient, None).map(|(passes, _)| passes)
}

/// `build_passes` に、背景のさらに奥の虹色の塊 (仕様 09 §9) を足せるもの。`deep` があるとき、合成のあと・ブルームの
/// 前に `DeepBackgroundPass` を置き、塊を動かす時計 (`DeepBackgroundClock`) も返す。
pub fn build_passes_with(
    gpu: &GpuContext,
    materials: &charon_core::material::MaterialTable,
    width: u32,
    height: u32,
    clear: wgpu::Color,
    ambient: f32,
    deep: Option<DeepBackgroundParams>,
) -> Result<(PassList, Option<DeepBackgroundClock>), String> {
    let mut passes = PassList::new(gpu, width, height, 4);
    let layer_count = (width.div_ceil(64) + 1) * (height.div_ceil(64) + 1);
    let cells = CellPass::new(gpu, materials, PassList::scene_format(), layer_count)
        .map_err(|err| err.to_string())?;
    passes.add(BackgroundPass::new(clear));
    passes
        .add(BackgroundLayerPass::new(gpu, materials, layer_count).map_err(|err| err.to_string())?);
    passes.add(cells);
    passes.add(EntityPass::new(gpu));
    passes.add(LightPass::new(gpu, passes.scene_size(), passes.light_format()));
    passes.add(CompositePass::with_ambient(gpu, ambient));
    let clock = deep.map(|params| {
        let (pass, clock) = DeepBackgroundPass::new(gpu, params);
        passes.add(pass);
        clock
    });
    // 滑らかな剛体 (仕様 05 §11)。奥の背景のあと、後処理の前。
    passes.add(SmoothBodyPass::new(gpu, materials, ambient));
    // 画像のスプライト (仕様 06 §10)。滑らかな剛体のあと、後処理の前。
    passes.add(SpritePass::new(gpu, ambient));
    passes
        .add(FullscreenPass::post_process(gpu, include_str!("../../charon-render/src/bloom.wgsl")));
    Ok((passes, clock))
}

/// 描き方が `Smooth` の剛体から、滑らかな描画 (仕様 05 §11) の入力を作る。`id` は剛体の添字。
pub(crate) fn smooth_sprites(physics: &PhysicsEngine) -> Vec<SmoothBodySprite<'_>> {
    physics
        .bodies
        .iter()
        .enumerate()
        .filter(|(_, body)| body.render == BodyRender::Smooth)
        .filter_map(|(index, body)| {
            let pose = physics.body_pose(index)?;
            Some(SmoothBodySprite {
                id: index as u64,
                width: body.width,
                height: body.height,
                cells: &body.cells,
                center: pose.center,
                origin: body.origin,
                angle: pose.angle,
            })
        })
        .collect()
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
/// `rainbow` が `Some(先頭の ID)` のときは、材質を各セルの x 座標で選ぶ虹色にする (仕様 09 §2)。
fn apply_pointer(
    world: &mut World,
    material: MaterialId,
    rainbow: Option<MaterialId>,
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
            let material = rainbow.map_or(material, |first| rainbow_material(first, cell.x));
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
                .with_title(if self.options.demo {
                    "Charon — demo"
                } else {
                    "Charon — live world"
                })
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
            let mut gui = Gui::new(
                gpu,
                &window,
                self.brush_material,
                self.options.demo.then(|| &**self.world.materials()),
            );
            if let Some(controls) = previous {
                gui.controls = controls;
            } else {
                gui.controls.rainbow = self.options.rainbow_sand;
                // 保存した設定 (仕様 10 §14)。
                gui.controls.ui_scale = self.settings.ui_scale;
                gui.controls.sprite_scale = self.settings.character_scale;
                gui.controls.light_quality = self.settings.light_quality();
                gui.controls.master_volume = self.settings.master_volume;
                gui.controls.sfx_volume = self.settings.sfx_volume;
                gui.controls.music_volume = self.settings.music_volume;
                gui.controls.bindings = self.keys.bindings().clone();
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
                    Ok(mut gpu) => {
                        // 初期化の途中で届いた `Resized` は、`self.gpu` がまだないため捨てられている。
                        // `GpuContext::new` に渡したのは `resumed` 時点のサイズ (0×0 のことがある) なので、
                        // 準備ができた時点の大きさで構成し直す。これがないとサーフェスが未構成のまま、
                        // 再描画が要求されずにループが止まる。
                        let size = window.inner_size();
                        if let Err(err) = gpu.resize(size.width, size.height) {
                            set_web_status(&format!("GPU の初期化に失敗: {err}"));
                            event_loop.exit();
                            return;
                        }
                        self.gpu = Some(gpu);
                    }
                    Err(err) => {
                        eprintln!("GPU の初期化に失敗: {err}");
                        set_web_status(&format!("GPU の初期化に失敗: {err}"));
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
                    let mut gui = Gui::new(gpu, &window, self.brush_material, None);
                    gui.controls.web_status = self.web_status.clone();
                    self.gui = Some(gui);
                }
                set_web_status("実行中 — ワールドの保存は Sandbox パネルから");
            }
        }
        let consumed = self.gui.as_mut().is_some_and(|gui| gui.on_event(&window, &event));
        // 画面のスティック・ボタンのタッチは、カメラ操作やブラシに渡さない (仕様 10 §10)。
        let pad_owned = match &event {
            WindowEvent::Touch(touch) if self.options.follow_player && !consumed => self
                .touch_pad
                .update(touch.id, touch.phase, [touch.location.x as f32, touch.location.y as f32]),
            _ => false,
        };
        let consumed = consumed || pad_owned;
        // キー割り当ての変更中は、次に押したキーを割り当てる (`Esc` は取り消し)。ほかの操作には使わない。
        if let WindowEvent::KeyboardInput { event: key, .. } = &event
            && key.state == ElementState::Pressed
            && let PhysicalKey::Code(code) = key.physical_key
            && let Some(gui) = self.gui.as_mut()
            && let Some(action) = gui.controls.rebinding
        {
            if code != KeyCode::Escape {
                gui.controls.bindings.bind(action, code);
            }
            gui.controls.rebinding = None;
            return;
        }
        if self.options.follow_player {
            // UI が文字入力などでキーを使うときは押下を無視する。離したキーは常に反映する。
            match self.keys.event(&event, !consumed) {
                Some(KeyCommand::ToggleFreeCamera) => {
                    self.free_camera = !self.free_camera;
                    self.weapon_down = false;
                    self.tool_down = false;
                }
                Some(KeyCommand::TogglePaint) => {
                    if let Some(gui) = self.gui.as_mut() {
                        gui.controls.paint = !gui.controls.paint;
                    }
                    self.weapon_down = false;
                }
                Some(KeyCommand::ToggleInventory) => {
                    if let Some(gui) = self.gui.as_mut() {
                        gui.controls.inventory_open = !gui.controls.inventory_open;
                    }
                }
                Some(KeyCommand::Pause) => {
                    if let Some(gui) = self.gui.as_mut() {
                        gui.controls.pause_menu = !gui.controls.pause_menu;
                        gui.controls.rebinding = None;
                    }
                    self.weapon_down = false;
                    self.tool_down = false;
                    // メニューを閉じたときに、設定を保存する。
                    self.save_settings();
                }
                None => {}
            }
            if matches!(event, WindowEvent::Focused(false)) {
                self.keys.release_all();
                self.touch_pad.release_all();
                self.weapon_down = false;
                self.tool_down = false;
            }
        }
        let gesture = if pad_owned {
            false
        } else if let (WindowEvent::Touch(touch), Some(gui)) = (&event, self.gui.as_mut()) {
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
            // 参照画面: `Ctrl` + ドラッグで矩形を選び、`Paste` が有効なら次のクリックで貼る。
            let mut handled = false;
            if !grabbing
                && !used_by_body
                && !consumed
                && !gesture
                && let Some(demo) = self.gui.as_mut().and_then(|gui| gui.demo.as_mut())
            {
                let at = match pointer {
                    PointerEvent::Down { pos, .. }
                    | PointerEvent::Move { pos, .. }
                    | PointerEvent::Up { pos, .. } => pointer_cell(pos, zoom, origin),
                };
                if let Some(cell) = at {
                    match pointer {
                        PointerEvent::Down { .. } if self.ctrl => {
                            demo.selection = Some([cell, cell]);
                            self.selecting = true;
                            handled = true;
                        }
                        PointerEvent::Move { .. } if self.selecting => {
                            if let Some([start, _]) = demo.selection {
                                demo.selection = Some([start, cell]);
                            }
                            handled = true;
                        }
                        PointerEvent::Up { .. } if self.selecting => {
                            self.selecting = false;
                            handled = true;
                        }
                        PointerEvent::Down { .. } if demo.paste_armed => {
                            if let Some(clip) = demo.clip.as_ref() {
                                paste(&mut self.world, clip, cell);
                            }
                            handled = true;
                        }
                        _ => {}
                    }
                }
            }
            // 離したときは、どの状況でも武器を止める。
            if matches!(pointer, PointerEvent::Up { .. }) {
                self.weapon_down = false;
            }
            let free = !grabbing && !used_by_body && !consumed && !gesture && !handled;
            if free && self.weapon_mode() {
                // 仕様 10 §9.5: 武器モードでは、左ボタンは武器 (ブラシは描かない)。
                if matches!(pointer, PointerEvent::Down { .. }) {
                    self.weapon_down = true;
                }
            } else if free {
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
                    let rainbow = self.gui.as_ref().and_then(|gui| gui.controls.rainbow);
                    self.operations.extend(apply_pointer(
                        &mut self.world,
                        selected,
                        rainbow,
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
            WindowEvent::ModifiersChanged(modifiers) => self.ctrl = modifiers.state().control_key(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = [position.x as f32, position.y as f32];
            }
            // 押下は UI が使っていなければ撃つ。離したときは常に止める。
            WindowEvent::MouseInput { button: MouseButton::Right, state, .. } => {
                self.tool_down = state == ElementState::Pressed && !consumed;
            }
            WindowEvent::MouseInput { button: MouseButton::Middle, state, .. } => {
                self.dig_down = state == ElementState::Pressed && !consumed;
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
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
                self.handle_demo_action(event_loop);
            }
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

    /// パネルは egui の標準フォントで描く。日本語は文字化け (□) になるので、状態表示は ASCII に保つ。
    #[test]
    fn panel_status_text_is_ascii() {
        for text in [
            NOT_SAVED.to_owned(),
            save_status_text(Ok(()), 1357),
            save_status_text(Err("cannot write persistent storage".to_owned()), 0),
            load_status_text(1357),
        ] {
            assert!(text.is_ascii(), "{text:?}");
        }
        assert_eq!(save_status_text(Ok(()), 7), "Saved (tick 7)");
        assert_eq!(load_status_text(7), "Loaded saved world (tick 7)");
        assert_eq!(save_status_text(Err("x".to_owned()), 0), "Not saved: x");
    }

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
            None,
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
            None,
            PointerEvent::Move { id: 0, pos: [40.0, 48.0] },
            4,
            [0.0, 0.0],
            1,
        );
        assert_eq!(world.get(pos), previous);
        apply_pointer(
            &mut world,
            selected,
            None,
            PointerEvent::Up { id: 0, pos: [60.0, 60.0] },
            4,
            [0.0, 0.0],
            1,
        );
        assert_ne!(world.get(CellPos::new(15, 15)).material, selected);
    }
}
