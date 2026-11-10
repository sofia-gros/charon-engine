//! セルのシミュレーション (仕様 02)。

mod movement;
pub mod neighborhood;
mod step;

pub use neighborhood::{MAX_REACH, Neighborhood};
pub use step::chunk_phase;
