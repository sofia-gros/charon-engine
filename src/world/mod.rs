//! 作者が内容を決める、多層の地形とブロック個体の基盤。
//!
//! 旧simulationの固定ゲーム用Worldとは独立した公開入口です。素材の寸法は
//! ContentRegistryで確定し、画像・window・壁時計を世界の正規状態へ持ち込みません。
//! 読み込まれていない区域と空のcellを区別し、編集は独立候補を検証してから公開します。

mod activity;
mod archive;
mod builder;
mod changes;
mod channels;
mod content;
mod generation;
mod jobs;
mod observer;
mod placement;
mod pressure;
mod pressure_push;
mod remap;
mod scalar_transport;
mod snapshot;
mod state;
mod storage;
mod terrain;
mod transport;

pub use activity::*;
pub use archive::*;
pub use builder::*;
pub use changes::*;
pub use channels::*;
pub use content::*;
pub use generation::*;
pub use jobs::*;
pub use observer::*;
pub use placement::*;
pub use pressure::*;
pub use pressure_push::*;
pub use remap::*;
pub use scalar_transport::*;
pub use storage::*;
pub use terrain::*;

use crate::foundation::{EngineError, ErrorCode};

fn invalid(context: &'static str) -> EngineError {
    EngineError::new(ErrorCode::InvalidValue, context)
}
fn conflict(context: &'static str) -> EngineError {
    EngineError::new(ErrorCode::Conflict, context)
}
