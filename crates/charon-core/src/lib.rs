//! Charon Engine のシミュレーション核。
//!
//! このクレートは描画・ウィンドウ・入出力に依存せず、ヘッドレスで動作する。
//! セル表現、材質、チャンク、落下砂シミュレーションをここに実装していく
//! (`docs/ROADMAP.md` の M1 以降)。

/// エンジンのバージョン文字列。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
