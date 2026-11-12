//! セルと材質を GPU に送るバイト列へ変換する (仕様 06 §3)。

use charon_core::cell::{Cell, FLAG_BURNING, FLAG_SMOOTH, MaterialId};
use charon_core::material::MaterialTable;

/// 1 セルを `Rgba8Uint` の 4 バイトへ変換する。
/// A の bit 0 は燃焼、bit 1〜7 は 16 度単位の表示温度。
/// `FLAG_SMOOTH` のセル (滑らかな剛体、仕様 05 §11) は、セルの描画では描かないので空のテクセルにする。
pub fn cell_to_texel(cell: Cell) -> [u8; 4] {
    if cell.has(FLAG_SMOOTH) {
        return [0; 4];
    }
    let [lo, hi] = cell.material.0.to_le_bytes();
    let displayed_temp = (cell.temp / 16).clamp(0, 127) as u8;
    let burning = u8::from(cell.has(FLAG_BURNING));
    [lo, hi, cell.shade, (displayed_temp << 1) | burning]
}

/// `Rgba8UnormSrgb` の色パレットと `Rgba8Unorm` の補助パレット。
#[derive(Debug, PartialEq, Eq)]
pub struct PaletteBytes {
    pub color: Vec<u8>,
    pub aux: Vec<u8>,
}

/// 材質 ID の全範囲をパレットへ格納する。各ピクセル 4 バイト。
pub fn palette_bytes(materials: &MaterialTable) -> PaletteBytes {
    let mut color = Vec::with_capacity(materials.len() * 4);
    let mut aux = Vec::with_capacity(materials.len() * 4);
    for index in 0..materials.len() {
        let mat = materials.get(MaterialId(index as u16));
        color.extend_from_slice(&mat.color);
        aux.extend_from_slice(&[
            mat.emission[0],
            mat.emission[1],
            mat.emission[2],
            mat.color_variation,
        ]);
    }
    PaletteBytes { color, aux }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_to_texel() {
        let cell = Cell {
            material: MaterialId(0x1234),
            flags: FLAG_BURNING,
            shade: 217,
            temp: 160,
            ..Cell::default()
        };
        assert_eq!(super::cell_to_texel(cell), [0x34, 0x12, 217, 21]);
        assert_eq!(super::cell_to_texel(Cell { temp: -500, ..cell })[3], 1);
        assert_eq!(super::cell_to_texel(Cell { temp: i16::MAX, flags: 0, ..cell })[3], 254);
        assert_eq!(super::cell_to_texel(Cell::default()), [0, 0, 0, 0]);
    }

    #[test]
    fn smooth_flag_hides_cell_texels() {
        let cell = Cell {
            material: MaterialId(0x1234),
            flags: FLAG_BURNING,
            shade: 217,
            temp: 160,
            ..Cell::default()
        };
        let smooth =
            Cell { flags: cell.flags | FLAG_SMOOTH | charon_core::cell::FLAG_BODY, ..cell };
        assert_eq!(
            super::cell_to_texel(smooth),
            [0, 0, 0, 0],
            "滑らかな剛体のセルはセルの描画では描かない"
        );
        // ほかのフラグ・温度の変換は変わらない (FLAG_BODY だけのセルは従来どおり描く)。
        let body = Cell { flags: cell.flags | charon_core::cell::FLAG_BODY, ..cell };
        assert_eq!(super::cell_to_texel(body), super::cell_to_texel(cell));
        assert_eq!(super::cell_to_texel(cell), [0x34, 0x12, 217, 21]);
    }

    #[test]
    fn palette_bytes() {
        let materials = MaterialTable::builtin().unwrap();
        let palettes = super::palette_bytes(&materials);
        assert_eq!(palettes.color.len(), materials.len() * 4);
        assert_eq!(palettes.aux.len(), materials.len() * 4);
        for i in 0..materials.len() {
            let mat = materials.get(MaterialId(i as u16));
            assert_eq!(&palettes.color[i * 4..i * 4 + 4], &mat.color);
            assert_eq!(
                &palettes.aux[i * 4..i * 4 + 4],
                &[mat.emission[0], mat.emission[1], mat.emission[2], mat.color_variation]
            );
        }
    }
}
