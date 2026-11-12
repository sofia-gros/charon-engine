//! 仕様 09: 参照デモ場面の生成。虹色の選び方、字形、洞窟の生成器。

use charon_core::cell::MaterialId;

/// 虹色の材質の数 (色相 30° 刻み)。
pub const RAINBOW_COUNT: i32 = 12;

/// 仕様 09 §2: セルの x 座標から虹色の添字 (0..=11) を求める。4 セルごとに色が変わる。
pub fn rainbow_index(x: i32) -> u16 {
    // rem_euclid は 0..=11 を返すので u16 に収まる。
    x.div_euclid(4).rem_euclid(RAINBOW_COUNT) as u16
}

/// 虹色の材質群の先頭 ID `first` から、`x` の位置の色の ID を返す。ID は連続している。
pub fn rainbow_material(first: MaterialId, x: i32) -> MaterialId {
    MaterialId(first.0.saturating_add(rainbow_index(x)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rainbow_index_is_periodic() {
        for x in -200..200 {
            let index = rainbow_index(x);
            assert!(index < 12, "x={x}");
            assert_eq!(index, rainbow_index(x + 48), "x={x}");
            assert_eq!(index, rainbow_index(x - 48), "x={x}");
        }
        // 4 セルごとに同じ色、境界で次の色。
        assert_eq!(rainbow_index(0), rainbow_index(3));
        assert_eq!(rainbow_index(4), rainbow_index(0) + 1);
        assert_eq!(rainbow_index(-1), 11);
        assert_eq!(rainbow_material(MaterialId(20), 8), MaterialId(22));
    }
}
