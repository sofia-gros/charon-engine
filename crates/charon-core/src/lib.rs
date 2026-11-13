//! Charon Engine のシミュレーション核。
//!
//! このクレートは描画・ウィンドウ・入出力に依存せず、ヘッドレスで動作する。
//! 仕様は `docs/spec/` にある。

pub mod cell;
pub mod chunk;
pub mod coords;
pub mod material;
pub mod query;
pub mod reaction;
pub mod rng;
pub mod sim;
pub mod testkit;
pub mod world;

/// エンジンのバージョン文字列。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 入力同期で比較する核の実際の並列化 feature 構成。
pub const PARALLEL_ENABLED: bool = cfg!(feature = "parallel");
