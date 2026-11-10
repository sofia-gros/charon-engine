//! Charon Engine のシミュレーション核。
//!
//! このクレートは描画・ウィンドウ・入出力に依存せず、ヘッドレスで動作する。
//! 仕様は `docs/spec/` にある。

pub mod cell;
pub mod coords;

/// エンジンのバージョン文字列。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
