//! 決定的な乱数 (仕様 01 §4)。
//!
//! 状態を持つ乱数生成器を使ってはいけない。乱数は必ず [`hash`] から得る。
//! 入力が同じなら、スレッド数や実行順に関係なく同じ値になる。

/// seed・tick・座標・salt から 32 ビットの乱数を作る。
///
/// `salt` は呼び出し箇所ごとに異なる定数 (`SALT_*`) にする。
pub fn hash(seed: u64, tick: u64, x: i32, y: i32, salt: u32) -> u32 {
    let mut z = seed
        ^ tick.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ ((x as u32 as u64) << 32 | (y as u32 as u64)).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ (salt as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) as u32
}

/// 確率 `p / 256` で true。
pub fn chance(h: u32, p: u8) -> bool {
    (h & 0xFF) < p as u32
}

/// make_cell: 色のばらつき。
pub const SALT_SHADE: u32 = 1;
/// make_cell / convert: 寿命。
pub const SALT_LIFE: u32 = 2;
/// make_cell: 流れる向きの初期値。
pub const SALT_FLOW: u32 = 3;
/// 行を走査する向き。
pub const SALT_SCAN: u32 = 4;
/// 液体の中を沈むときの抵抗。
pub const SALT_DRAG: u32 = 5;
/// 落下の歩数の端数。
pub const SALT_FALL: u32 = 6;
/// 粉体が斜めに落ちる向き。
pub const SALT_DIAG: u32 = 7;
/// 隣の粉体を起こすか。
pub const SALT_WAKE: u32 = 8;
/// 液体・気体が移動を見送るか。
pub const SALT_VISC: u32 = 9;
/// 反応を試す方向。
pub const SALT_REACT_DIR: u32 = 10;
/// 反応が起きるか。
pub const SALT_REACT: u32 = 11;
/// 炎を出す方向。
pub const SALT_FIRE_DIR: u32 = 12;
/// 炎を出すか。
pub const SALT_FIRE: u32 = 13;
/// 着地した粉体がその場に止まるか (摩擦)。
pub const SALT_STICK: u32 = 14;
/// 地表の 1 次元値ノイズ。
pub const SALT_TERRAIN: u32 = 15;
/// 洞窟の 2 次元値ノイズ。
pub const SALT_CAVE: u32 = 16;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_input_same_output() {
        assert_eq!(hash(1, 2, 3, -4, 5), hash(1, 2, 3, -4, 5));
    }

    #[test]
    fn each_input_changes_output() {
        let base = hash(1, 2, 3, 4, 5);
        assert_ne!(base, hash(9, 2, 3, 4, 5));
        assert_ne!(base, hash(1, 9, 3, 4, 5));
        assert_ne!(base, hash(1, 2, 9, 4, 5));
        assert_ne!(base, hash(1, 2, 3, 9, 5));
        assert_ne!(base, hash(1, 2, 3, 4, 9));
        // x と y を入れ替えると別の値になる
        assert_ne!(hash(1, 2, 3, 4, 5), hash(1, 2, 4, 3, 5));
    }

    /// 他の言語の実装 (tools/prototype/fluid.py) と同じ式であることを固定する。
    #[test]
    fn known_values() {
        assert_eq!(hash(0, 0, 0, 0, 0), 0);
        assert_eq!(hash(1, 0, 0, 0, 0), 0x100B_05E5);
    }

    #[test]
    fn chance_bounds() {
        for h in 0..1000u32 {
            assert!(!chance(h, 0));
        }
    }

    #[test]
    fn chance_is_roughly_fair() {
        let hits = (0..10_000).filter(|&i| chance(hash(7, 0, i, 0, 0), 128)).count();
        assert!((4_500..=5_500).contains(&hits), "hits = {hits}");
    }
}
