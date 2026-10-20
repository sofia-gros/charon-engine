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
mod material_accounting;
mod material_components;
mod material_forces;
mod material_forces_legacy;
mod material_inventory;
mod material_mixtures;
mod material_motion;
mod material_motion_job;
mod material_particle_sleep;
mod material_particles;
mod material_phase;
mod material_provider_sleep;
mod material_receipts;
mod material_scalar_sleep;
mod material_sleep;
mod observer;
mod placement;
mod pressure;
mod pressure_job;
mod pressure_push;
mod pressure_sleep;
mod remap;
mod rigid_bodies;
mod scalar_transport;
mod snapshot;
mod state;
mod storage;
mod terrain;
mod thermal_reactions;
mod transport;

pub use activity::*;
pub use archive::*;
pub use builder::*;
pub use changes::*;
pub use channels::*;
pub use content::*;
pub use generation::*;
pub use jobs::*;
pub use material_accounting::*;
pub use material_components::*;
pub use material_forces::*;
pub use material_forces_legacy::*;
pub use material_inventory::*;
pub use material_mixtures::*;
pub use material_motion::*;
pub use material_motion_job::*;
pub use material_particle_sleep::*;
pub use material_particles::*;
pub use material_phase::*;
pub use material_provider_sleep::*;
pub use material_receipts::*;
pub use material_scalar_sleep::*;
pub use material_sleep::*;
pub use observer::*;
pub use placement::*;
pub use pressure::*;
pub use pressure_job::*;
pub use pressure_push::*;
pub use pressure_sleep::*;
pub use remap::*;
pub use rigid_bodies::*;
pub use scalar_transport::*;
pub use storage::*;
pub use terrain::*;
pub use thermal_reactions::*;

use crate::foundation::{EngineError, ErrorCode};

fn invalid(context: &'static str) -> EngineError {
    EngineError::new(ErrorCode::InvalidValue, context)
}
fn conflict(context: &'static str) -> EngineError {
    EngineError::new(ErrorCode::Conflict, context)
}
