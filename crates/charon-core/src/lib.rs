//! Charon Engine のシミュレーション核。
//!
//! このクレートは描画・ウィンドウ・入出力に依存せず、ヘッドレスで動作する。
//! 仕様は `docs/spec/` にある。

pub mod cell;
pub mod chunk;
pub mod coords;
pub mod material;
pub mod rng;
pub mod world;

/// エンジンのバージョン文字列。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
