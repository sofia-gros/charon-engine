//! 仕様 07 §4: 前景とは独立した背景セルと、名前に基づく保存。

use std::collections::{BTreeMap, HashMap};
#[cfg(not(target_arch = "wasm32"))]
use std::fs;
use std::io;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

use charon_core::cell::MaterialId;
use charon_core::chunk::{Rect, add_rect};
use charon_core::coords::{CHUNK_AREA, CellPos, ChunkPos};
use charon_core::material::MaterialTable;

use crate::RegionError;
#[cfg(not(target_arch = "wasm32"))]
use crate::region::atomic_replace;

const MAGIC: &[u8; 4] = b"CHBG";
const VERSION: u16 = 1;
const CHUNK_BYTES: usize = 8 + CHUNK_AREA * 2;

/// 1 チャンク分の背景。物理・熱・休眠には参加しない。
#[derive(Clone, Debug)]
pub struct BackgroundChunk {
    pub cells: Box<[MaterialId; CHUNK_AREA]>,
    pub render_dirty: Option<Rect>,
}

impl Default for BackgroundChunk {
    fn default() -> Self {
        Self { cells: Box::new([MaterialId::EMPTY; CHUNK_AREA]), render_dirty: None }
    }
}

#[derive(Clone, Debug, Default)]
pub struct BackgroundWorld {
    chunks: BTreeMap<ChunkPos, BackgroundChunk>,
}

#[derive(Debug)]
pub enum BackgroundError {
    Io(io::Error),
    Replace(RegionError),
    InvalidData,
    NewerVersion(u16),
    InvalidMaterial(MaterialId),
    TooLarge,
}

impl std::fmt::Display for BackgroundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "背景の入出力に失敗: {err}"),
            Self::Replace(err) => write!(f, "背景の置き換えに失敗: {err}"),
            Self::InvalidData => write!(f, "背景の保存データが壊れています"),
            Self::NewerVersion(version) => write!(f, "未対応の新しい背景形式: {version}"),
            Self::InvalidMaterial(id) => write!(f, "保存できない背景材質: {}", id.0),
            Self::TooLarge => write!(f, "背景のチャンク数が u32 の上限を超えました"),
        }
    }
}

impl std::error::Error for BackgroundError {}

impl BackgroundWorld {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, pos: CellPos) -> MaterialId {
        self.chunks
            .get(&pos.chunk())
            .map_or(MaterialId::EMPTY, |chunk| chunk.cells[pos.local_index()])
    }

    /// 同じ値なら何も書かず、変更時だけ背景の描画 dirty を付ける。
    pub fn set(&mut self, pos: CellPos, material: MaterialId) {
        if self.get(pos) == material {
            return;
        }
        let chunk = self.chunks.entry(pos.chunk()).or_default();
        chunk.cells[pos.local_index()] = material;
        let x = (pos.x & 63) as u8;
        let y = (pos.y & 63) as u8;
        add_rect(&mut chunk.render_dirty, Rect::point(x, y));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ChunkPos, &BackgroundChunk)> {
        self.chunks.iter()
    }

    pub fn chunk(&self, pos: ChunkPos) -> Option<&BackgroundChunk> {
        self.chunks.get(&pos)
    }

    pub fn take_render_dirty(&mut self, pos: ChunkPos) -> Option<Rect> {
        self.chunks.get_mut(&pos).and_then(|chunk| chunk.render_dirty.take())
    }

    /// world.ron の材質名順の u16 添字で、座標昇順に符号化する。
    pub fn to_bytes(
        &self,
        materials: &MaterialTable,
        saved_names: &[String],
    ) -> Result<Vec<u8>, BackgroundError> {
        let count = u32::try_from(self.chunks.len()).map_err(|_| BackgroundError::TooLarge)?;
        let names: HashMap<&str, u16> = saved_names
            .iter()
            .enumerate()
            .filter_map(|(index, name)| {
                u16::try_from(index).ok().map(|index| (name.as_str(), index))
            })
            .collect();
        let mut bytes = Vec::with_capacity(10 + self.chunks.len() * CHUNK_BYTES);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        for (pos, chunk) in &self.chunks {
            bytes.extend_from_slice(&pos.x.to_le_bytes());
            bytes.extend_from_slice(&pos.y.to_le_bytes());
            for &material in chunk.cells.iter() {
                if usize::from(material.0) >= materials.len() {
                    return Err(BackgroundError::InvalidMaterial(material));
                }
                let Some(&index) = names.get(materials.get(material).name.as_str()) else {
                    return Err(BackgroundError::InvalidMaterial(material));
                };
                bytes.extend_from_slice(&index.to_le_bytes());
            }
        }
        Ok(bytes)
    }

    /// ネイティブでは一時ファイルから原子的に置換する。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save(
        &self,
        root: &Path,
        materials: &MaterialTable,
        saved_names: &[String],
    ) -> Result<(), BackgroundError> {
        let bytes = self.to_bytes(materials, saved_names)?;
        atomic_replace(&root.join("background.bin"), &bytes).map_err(BackgroundError::Replace)
    }

    /// 保存がなければ空の背景。未知の材質名は EMPTY に置く。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load(
        root: &Path,
        materials: &MaterialTable,
        saved_names: &[String],
    ) -> Result<Self, BackgroundError> {
        let bytes = match fs::read(root.join("background.bin")) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(err) => return Err(BackgroundError::Io(err)),
        };
        Self::from_bytes(&bytes, materials, saved_names)
    }

    /// ブラウザーの保存アダプターが渡したバイト列を検証して復元する。
    pub fn from_bytes(
        bytes: &[u8],
        materials: &MaterialTable,
        saved_names: &[String],
    ) -> Result<Self, BackgroundError> {
        let header = bytes.get(..10).ok_or(BackgroundError::InvalidData)?;
        if &header[..4] != MAGIC {
            return Err(BackgroundError::InvalidData);
        }
        let version = u16::from_le_bytes([header[4], header[5]]);
        if version > VERSION {
            return Err(BackgroundError::NewerVersion(version));
        }
        if version != VERSION {
            return Err(BackgroundError::InvalidData);
        }
        let count = u32::from_le_bytes([header[6], header[7], header[8], header[9]]) as usize;
        let expected = count.checked_mul(CHUNK_BYTES).and_then(|n| n.checked_add(10));
        if expected != Some(bytes.len()) {
            return Err(BackgroundError::InvalidData);
        }
        let remap: Vec<MaterialId> = saved_names
            .iter()
            .map(|name| materials.id(name).unwrap_or(MaterialId::EMPTY))
            .collect();
        let mut chunks = BTreeMap::new();
        for data in bytes[10..].as_chunks::<CHUNK_BYTES>().0 {
            let x = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            let y = i32::from_le_bytes([data[4], data[5], data[6], data[7]]);
            let mut chunk = BackgroundChunk::default();
            for (cell, value) in chunk.cells.iter_mut().zip(data[8..].as_chunks::<2>().0) {
                let index = usize::from(u16::from_le_bytes([value[0], value[1]]));
                *cell = *remap.get(index).ok_or(BackgroundError::InvalidData)?;
            }
            chunk.render_dirty = Some(Rect { min_x: 0, min_y: 0, max_x: 63, max_y: 63 });
            if chunks.insert(ChunkPos::new(x, y), chunk).is_some() {
                return Err(BackgroundError::InvalidData);
            }
        }
        Ok(Self { chunks })
    }
}
