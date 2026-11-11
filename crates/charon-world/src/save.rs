//! 仕様 04 §3: `world.ron` に保存する世界のメタデータ。

use std::fmt;
#[cfg(not(target_arch = "wasm32"))]
use std::fs;
use std::io;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

use charon_core::material::MaterialTable;
use serde::{Deserialize, Serialize};

use crate::RegionError;
use crate::material_names;
#[cfg(not(target_arch = "wasm32"))]
use crate::region::atomic_replace;

const VERSION: u16 = 1;

/// 保存時の材質名順を保持して ID の並び替えに対応する。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorldMeta {
    pub version: u16,
    pub seed: u64,
    pub tick: u64,
    pub materials: Vec<String>,
}

#[derive(Debug)]
pub enum SaveError {
    Io(io::Error),
    Parse(ron::error::SpannedError),
    Serialize(ron::Error),
    Region(RegionError),
    NewerVersion(u16),
    UnsupportedVersion(u16),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "ワールド情報を読めません: {err}"),
            Self::Parse(err) => write!(f, "ワールド情報が壊れています: {err}"),
            Self::Serialize(err) => write!(f, "ワールド情報を符号化できません: {err}"),
            Self::Region(err) => write!(f, "ワールド情報を保存できません: {err}"),
            Self::NewerVersion(version) => write!(f, "未対応の新しいワールド形式: {version}"),
            Self::UnsupportedVersion(version) => write!(f, "未対応の古いワールド形式: {version}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl WorldMeta {
    pub fn new(seed: u64, materials: &MaterialTable) -> Self {
        Self { version: VERSION, seed, tick: 0, materials: material_names(materials) }
    }

    /// 保存がなければ None。新しい版や壊れたファイルは明示的にエラーにする。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load(root: &Path) -> Result<Option<Self>, SaveError> {
        let text = match fs::read_to_string(root.join("world.ron")) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(SaveError::Io(err)),
        };
        Self::from_ron(&text).map(Some)
    }

    pub fn from_ron(text: &str) -> Result<Self, SaveError> {
        let metadata: Self = ron::from_str(text).map_err(SaveError::Parse)?;
        if metadata.version > VERSION {
            return Err(SaveError::NewerVersion(metadata.version));
        }
        if metadata.version != VERSION {
            return Err(SaveError::UnsupportedVersion(metadata.version));
        }
        Ok(metadata)
    }

    pub fn to_ron(&self) -> Result<String, SaveError> {
        ron::ser::to_string(self).map_err(SaveError::Serialize)
    }

    /// チャンクの書き込みが完了した後、同じ保存ルートへ原子的に書く。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save(&self, root: &Path) -> Result<(), SaveError> {
        let bytes = self.to_ron()?;
        atomic_replace(&root.join("world.ron"), bytes.as_bytes()).map_err(SaveError::Region)
    }
}
