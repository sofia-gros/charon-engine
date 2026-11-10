//! セル (仕様 01 §2)。
//!
//! 1 セルは 8 バイトの `Copy` 型。ヒープを指すフィールドを追加してはいけない。

/// 材質の ID。`MaterialTable` の添字。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
#[repr(transparent)]
pub struct MaterialId(pub u16);

impl MaterialId {
    /// 何もない (空気)。
    pub const EMPTY: MaterialId = MaterialId(0);
    /// 読み込まれていない領域。動かず壊れない固体。
    pub const BOUNDARY: MaterialId = MaterialId(1);
}

/// 格子の 1 マス。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(C)]
pub struct Cell {
    pub material: MaterialId,
    /// `FLAG_*` のビット和。
    pub flags: u8,
    /// 色のばらつき。生成時に乱数で決め、以後変えない。
    pub shade: u8,
    /// 温度。単位は摂氏 1 度。
    pub temp: i16,
    /// 落下速度。単位は 1/16 セル毎 tick。
    pub vy: i8,
    /// 残り寿命。単位は 4 tick。0 は「寿命なし」。
    pub life: u8,
}

const _: () = assert!(std::mem::size_of::<Cell>() == 8);

/// この tick で更新済みかの判定に使う (仕様 02 §3)。
pub const FLAG_PARITY: u8 = 1 << 0;
/// 動いている途中。粉体は仕様 02 §7、液体・気体は仕様 02 §8 の「勢い」。
pub const FLAG_FALLING: u8 = 1 << 1;
/// 液体・気体の水平移動の向き。立っていれば +x。
pub const FLAG_FLOW_RIGHT: u8 = 1 << 2;
/// 燃焼中 (仕様 03 §4)。
pub const FLAG_BURNING: u8 = 1 << 3;
/// 剛体が書き込んだセル (仕様 05)。保存しない。
pub const FLAG_BODY: u8 = 1 << 4;
/// 液体・気体が水平移動で 1 回はね返った (仕様 02 §8)。
pub const FLAG_BOUNCED: u8 = 1 << 5;

impl Cell {
    /// 読み込まれていない領域を表すセル。
    pub const BOUNDARY: Cell = Cell {
        material: MaterialId::BOUNDARY,
        flags: 0,
        shade: 0,
        temp: 0,
        vy: 0,
        life: 0,
    };

    /// `flag` のビットがすべて立っているか。
    pub const fn has(self, flag: u8) -> bool {
        self.flags & flag == flag
    }

    /// `flag` を立てた (`on == true`) または下ろしたセルを返す。
    #[must_use]
    pub const fn with(mut self, flag: u8, on: bool) -> Cell {
        if on {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        let c = Cell::default();
        assert_eq!(c.material, MaterialId::EMPTY);
        assert_eq!((c.flags, c.shade, c.temp, c.vy, c.life), (0, 0, 0, 0, 0));
    }

    #[test]
    fn flags_do_not_overlap() {
        let all = [
            FLAG_PARITY,
            FLAG_FALLING,
            FLAG_FLOW_RIGHT,
            FLAG_BURNING,
            FLAG_BODY,
            FLAG_BOUNCED,
        ];
        let mut seen = 0u8;
        for f in all {
            assert_eq!(f.count_ones(), 1);
            assert_eq!(seen & f, 0);
            seen |= f;
        }
    }

    #[test]
    fn with_sets_and_clears() {
        let c = Cell::default().with(FLAG_FALLING, true);
        assert!(c.has(FLAG_FALLING));
        assert!(!c.with(FLAG_FALLING, false).has(FLAG_FALLING));
    }
}
