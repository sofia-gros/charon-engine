//! Scene・入力・作者システムを固定更新へ統合する実行基盤。
//!
//! Worldの所有はSceneから独立しています。headlessでも同じ更新経路を使い、
//! ウィンドウの有無やrenderの回数で正規時刻を進めません。

pub mod clock;
pub mod config;
mod engine;
mod host;
pub mod input;
mod scene;
pub mod schedule;

pub use engine::{
    AppBuilder, Engine, EngineBuilder, HeadlessRuntime, MaterialAccountingSubmission, Plugin,
    RenderCapabilities, ServiceRegistry, StepResult, WorldAccess, WorldView,
};
pub use host::{DesktopGpu, DesktopWindow};
pub use scene::{FrameContext, LoadContext, Scene, SceneContext, SceneState, StopContext};
