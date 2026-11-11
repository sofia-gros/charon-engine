//! ネイティブのリージョン保存と Web メモリ保存で共有するエラー。

use std::fmt;
use std::io;

use crate::ChunkCodecError;

/// リージョンまたは圧縮済みチャンクを読めない・保存できない理由。
#[derive(Debug)]
pub enum RegionError {
    Io(io::Error),
    Codec(ChunkCodecError),
    InvalidHeader,
    NewerVersion(u16),
    TooLarge,
}

impl fmt::Display for RegionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "リージョンの入出力に失敗: {err}"),
            Self::Codec(err) => write!(f, "チャンクの符号化に失敗: {err}"),
            Self::InvalidHeader => write!(f, "リージョンのヘッダーが壊れています"),
            Self::NewerVersion(version) => write!(f, "未対応の新しいリージョン形式: {version}"),
            Self::TooLarge => write!(f, "リージョンが u32 のアドレス範囲を超えました"),
        }
    }
}

impl std::error::Error for RegionError {}

impl From<io::Error> for RegionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ChunkCodecError> for RegionError {
    fn from(error: ChunkCodecError) -> Self {
        Self::Codec(error)
    }
}
