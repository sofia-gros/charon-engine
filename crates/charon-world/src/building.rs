//! 仕様 07 §4: 前景セルと背景セルへの決定的な建築操作。

use charon_core::cell::{Cell, FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::world::World;

use crate::background::BackgroundWorld;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Foreground,
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildOp {
    pub pos: CellPos,
    pub layer: Layer,
    pub material: MaterialId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildError {
    InvalidMaterial(MaterialId),
    Blocked,
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMaterial(id) => write!(f, "未定義の建築材質: {}", id.0),
            Self::Blocked => write!(f, "建築先は空ではないか、未読み込みです"),
        }
    }
}

impl std::error::Error for BuildError {}

/// 値が変わったときだけ書き込む。true は変更あり、false は同じ値。
pub fn apply_build(
    world: &mut World,
    background: &mut BackgroundWorld,
    op: BuildOp,
) -> Result<bool, BuildError> {
    if usize::from(op.material.0) >= world.materials().len() {
        return Err(BuildError::InvalidMaterial(op.material));
    }
    match op.layer {
        Layer::Background => {
            let old = background.get(op.pos);
            if old == op.material {
                return Ok(false);
            }
            background.set(op.pos, op.material);
        }
        Layer::Foreground => {
            let old = world.get(op.pos);
            if old.material == MaterialId::BOUNDARY || old.has(FLAG_BODY) {
                return Err(BuildError::Blocked);
            }
            if op.material != MaterialId::EMPTY && old.material != MaterialId::EMPTY {
                return Err(BuildError::Blocked);
            }
            if op.material == MaterialId::EMPTY && old == Cell::default() {
                return Ok(false);
            }
            let cell = if op.material == MaterialId::EMPTY {
                Cell::default()
            } else {
                world.make_cell(op.material, op.pos)
            };
            world.set(op.pos, cell);
        }
    }
    Ok(true)
}
