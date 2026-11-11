//! Web のストレージアダプターに渡す、前景・背景・剛体のバイト列。
//! ブラウザー API とファイル API を使わず、既存の各保存形式を包む。

use std::collections::BTreeMap;
use std::fmt;

use charon_core::coords::ChunkPos;

use crate::save::{SaveError, WorldMeta};

const MAGIC: &[u8; 4] = b"CHWB";
const VERSION: u16 = 1;

#[derive(Debug)]
pub enum WebSaveError {
    InvalidData,
    NewerVersion(u16),
    TooLarge,
    Metadata(SaveError),
}

impl fmt::Display for WebSaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData => write!(f, "Web の保存データが壊れています"),
            Self::NewerVersion(version) => write!(f, "未対応の新しい Web 保存形式: {version}"),
            Self::TooLarge => write!(f, "Web の保存データが大きすぎます"),
            Self::Metadata(err) => write!(f, "ワールド情報を読めません: {err}"),
        }
    }
}

impl std::error::Error for WebSaveError {}

/// 同一 world.ron の材質名一覧で、個別のバイナリ保存形式を復元する。
#[derive(Debug)]
pub struct WebSnapshot {
    pub metadata: WorldMeta,
    pub chunks: BTreeMap<ChunkPos, Vec<u8>>,
    pub background: Vec<u8>,
    pub bodies: Vec<u8>,
}

fn length(bytes: &[u8]) -> Result<u32, WebSaveError> {
    u32::try_from(bytes.len()).map_err(|_| WebSaveError::TooLarge)
}

fn write_blob(output: &mut Vec<u8>, blob: &[u8]) -> Result<(), WebSaveError> {
    output.extend_from_slice(&length(blob)?.to_le_bytes());
    output.extend_from_slice(blob);
    Ok(())
}

fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8], WebSaveError> {
    let (value, rest) = bytes.split_at_checked(length).ok_or(WebSaveError::InvalidData)?;
    *bytes = rest;
    Ok(value)
}

fn take_u32(bytes: &mut &[u8]) -> Result<u32, WebSaveError> {
    let data: [u8; 4] = take(bytes, 4)?.try_into().map_err(|_| WebSaveError::InvalidData)?;
    Ok(u32::from_le_bytes(data))
}

fn take_blob<'a>(bytes: &mut &'a [u8]) -> Result<&'a [u8], WebSaveError> {
    let length = take_u32(bytes)? as usize;
    take(bytes, length)
}

impl WebSnapshot {
    pub fn encode(&self) -> Result<Vec<u8>, WebSaveError> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        write_blob(&mut bytes, self.metadata.to_ron().map_err(WebSaveError::Metadata)?.as_bytes())?;
        write_blob(&mut bytes, &self.background)?;
        write_blob(&mut bytes, &self.bodies)?;
        bytes.extend_from_slice(
            &u32::try_from(self.chunks.len()).map_err(|_| WebSaveError::TooLarge)?.to_le_bytes(),
        );
        for (pos, chunk) in &self.chunks {
            bytes.extend_from_slice(&pos.x.to_le_bytes());
            bytes.extend_from_slice(&pos.y.to_le_bytes());
            write_blob(&mut bytes, chunk)?;
        }
        Ok(bytes)
    }

    pub fn decode(mut bytes: &[u8]) -> Result<Self, WebSaveError> {
        if take(&mut bytes, 4)? != MAGIC {
            return Err(WebSaveError::InvalidData);
        }
        let version_bytes: [u8; 2] =
            take(&mut bytes, 2)?.try_into().map_err(|_| WebSaveError::InvalidData)?;
        let version = u16::from_le_bytes(version_bytes);
        if version > VERSION {
            return Err(WebSaveError::NewerVersion(version));
        }
        if version != VERSION {
            return Err(WebSaveError::InvalidData);
        }
        let meta = take_blob(&mut bytes)?;
        let metadata =
            WorldMeta::from_ron(std::str::from_utf8(meta).map_err(|_| WebSaveError::InvalidData)?)
                .map_err(WebSaveError::Metadata)?;
        let background = take_blob(&mut bytes)?.to_vec();
        let bodies = take_blob(&mut bytes)?.to_vec();
        let count = take_u32(&mut bytes)? as usize;
        if count > bytes.len() / 12 {
            return Err(WebSaveError::InvalidData);
        }
        let mut chunks = BTreeMap::new();
        for _ in 0..count {
            let x = i32::from_le_bytes(take_u32(&mut bytes)?.to_le_bytes());
            let y = i32::from_le_bytes(take_u32(&mut bytes)?.to_le_bytes());
            let chunk = take_blob(&mut bytes)?.to_vec();
            if chunks.insert(ChunkPos::new(x, y), chunk).is_some() {
                return Err(WebSaveError::InvalidData);
            }
        }
        if !bytes.is_empty() {
            return Err(WebSaveError::InvalidData);
        }
        Ok(Self { metadata, chunks, background, bodies })
    }
}
