//! 1 チャンクとその周囲 8 チャンク (仕様 02 §2)。
//!
//! 同じフェーズのチャンクは 3 チャンクずつ離れているので、それぞれの `Neighborhood` は
//! 1 つもチャンクを共有しない。そのため 9 チャンクの所有権を丸ごと持てる。

use crate::cell::{Cell, FLAG_PARITY};
use crate::chunk::{Chunk, add_rect, wake_region};
use crate::coords::{CHUNK_MASK, CHUNK_SHIFT, ChunkPos};
use crate::material::MaterialTable;

/// 1 セルの更新が読み書きしてよい距離 (セル)。
pub const MAX_REACH: i32 = 32;

/// 中心チャンクと周囲 8 チャンク。
///
/// 座標は「中心チャンクの左上を (0, 0) とするローカル座標」で、有効範囲は x, y とも `-64..128`。
#[derive(Debug)]
pub struct Neighborhood<'a> {
    pub center: ChunkPos,
    /// 添字 = `(dx + 1) + 3 * (dy + 1)`。
    chunks: [Option<Box<Chunk>>; 9],
    materials: &'a MaterialTable,
    /// 現在の tick の `FLAG_PARITY` の値 (`tick & 1`)。
    parity: bool,
}

impl<'a> Neighborhood<'a> {
    pub fn new(
        center: ChunkPos,
        chunks: [Option<Box<Chunk>>; 9],
        materials: &'a MaterialTable,
        tick: u64,
    ) -> Self {
        Self {
            center,
            chunks,
            materials,
            parity: tick & 1 == 1,
        }
    }

    /// チャンクを取り出す (ワールドへ戻すため)。添字は [`Neighborhood::slot_pos`] で位置に変換する。
    pub fn into_chunks(self) -> [Option<Box<Chunk>>; 9] {
        self.chunks
    }

    /// 添字 `slot` (0..9) のチャンクの位置。
    pub fn slot_pos(center: ChunkPos, slot: usize) -> ChunkPos {
        ChunkPos::new(
            center.x + (slot % 3) as i32 - 1,
            center.y + (slot / 3) as i32 - 1,
        )
    }

    pub fn materials(&self) -> &'a MaterialTable {
        self.materials
    }

    /// 現在の tick の parity。
    pub fn parity(&self) -> bool {
        self.parity
    }

    /// 中心チャンク。
    pub fn center_chunk(&self) -> Option<&Chunk> {
        self.chunks[4].as_deref()
    }

    /// セルを読む。チャンクがなければ `BOUNDARY`。
    pub fn get(&self, x: i32, y: i32) -> Cell {
        let Some((slot, lx, ly)) = locate(x, y) else {
            return Cell::BOUNDARY;
        };
        match &self.chunks[slot] {
            Some(chunk) => chunk.get(lx, ly),
            None => Cell::BOUNDARY,
        }
    }

    /// セルを書き込み、周囲 5×5 を起こす (仕様 01 §6)。チャンクがなければ何もしない。
    pub fn set(&mut self, x: i32, y: i32, cell: Cell) {
        let Some((slot, lx, ly)) = locate(x, y) else {
            return;
        };
        let Some(chunk) = &mut self.chunks[slot] else {
            return;
        };
        chunk.write(lx, ly, cell, self.materials);
        self.keep_awake(x, y);
    }

    /// 2 つのセルを入れ替える。両方を「この tick で更新済み」にする。
    pub fn swap(&mut self, ax: i32, ay: i32, bx: i32, by: i32) {
        let a = self.get(ax, ay).with(FLAG_PARITY, self.parity);
        let b = self.get(bx, by).with(FLAG_PARITY, self.parity);
        self.set(ax, ay, b);
        self.set(bx, by, a);
    }

    /// セルの値は変えずに、周囲 5×5 を次の tick の処理範囲に加える。
    pub fn keep_awake(&mut self, x: i32, y: i32) {
        let chunks = &mut self.chunks;
        wake_region(x, y, |dcx, dcy, rect| {
            if (-1..=1).contains(&dcx)
                && (-1..=1).contains(&dcy)
                && let Some(c) = &mut chunks[((dcx + 1) + 3 * (dcy + 1)) as usize]
            {
                add_rect(&mut c.next_dirty, rect);
            }
        });
    }

    /// セルを「この tick で更新済み」にする。状態の変化ではないので dirty は付けない。
    pub fn mark_parity(&mut self, x: i32, y: i32) {
        let Some((slot, lx, ly)) = locate(x, y) else {
            return;
        };
        if let Some(chunk) = &mut self.chunks[slot] {
            chunk.write_parity(lx, ly, self.parity);
        }
    }
}

/// ローカル座標を (チャンクの添字, チャンク内 x, チャンク内 y) に変換する。
/// 有効範囲 (`-64..128`) の外は `None`。
fn locate(x: i32, y: i32) -> Option<(usize, u8, u8)> {
    let (cx, cy) = (x >> CHUNK_SHIFT, y >> CHUNK_SHIFT);
    if !(-1..=1).contains(&cx) || !(-1..=1).contains(&cy) {
        debug_assert!(false, "Neighborhood の範囲外: ({x}, {y})");
        return None;
    }
    Some((
        ((cx + 1) + 3 * (cy + 1)) as usize,
        (x & CHUNK_MASK) as u8,
        (y & CHUNK_MASK) as u8,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{FLAG_FALLING, MaterialId};
    use crate::chunk::Rect;

    fn full(materials: &MaterialTable, tick: u64) -> Neighborhood<'_> {
        Neighborhood::new(
            ChunkPos::new(0, 0),
            std::array::from_fn(|_| Some(Box::new(Chunk::new()))),
            materials,
            tick,
        )
    }

    fn cell(id: u16) -> Cell {
        Cell {
            material: MaterialId(id),
            ..Cell::default()
        }
    }

    #[test]
    fn coordinates_reach_the_right_chunk() {
        let m = MaterialTable::builtin().unwrap();
        let mut nb = full(&m, 0);
        let cases = [
            (-64, -64, 0, 0, 0),
            (-1, 0, 3, 63, 0),
            (0, 0, 4, 0, 0),
            (127, 127, 8, 63, 63),
        ];
        for (i, &(x, y, _, _, _)) in cases.iter().enumerate() {
            nb.set(x, y, cell(10 + i as u16));
        }
        for (i, &(x, y, _, _, _)) in cases.iter().enumerate() {
            assert_eq!(nb.get(x, y).material, MaterialId(10 + i as u16));
        }
        let chunks = nb.into_chunks();
        for (i, &(_, _, slot, lx, ly)) in cases.iter().enumerate() {
            let c = chunks[slot].as_ref().unwrap();
            assert_eq!(
                c.get(lx, ly).material,
                MaterialId(10 + i as u16),
                "case {i}"
            );
        }
    }

    #[test]
    fn missing_chunk_is_boundary() {
        let m = MaterialTable::builtin().unwrap();
        let mut chunks: [Option<Box<Chunk>>; 9] =
            std::array::from_fn(|_| Some(Box::new(Chunk::new())));
        chunks[5] = None; // 右隣
        let mut nb = Neighborhood::new(ChunkPos::new(0, 0), chunks, &m, 0);
        assert_eq!(nb.get(64, 0).material, MaterialId::BOUNDARY);
        nb.set(64, 0, cell(5)); // 何も起きない
        assert_eq!(nb.get(64, 0).material, MaterialId::BOUNDARY);
    }

    #[test]
    fn swap_exchanges_and_marks_parity() {
        let m = MaterialTable::builtin().unwrap();
        let mut nb = full(&m, 1);
        nb.set(63, 10, cell(7).with(FLAG_FALLING, true));
        nb.swap(63, 10, 64, 10);
        assert_eq!(nb.get(63, 10).material, MaterialId::EMPTY);
        let moved = nb.get(64, 10);
        assert_eq!(moved.material, MaterialId(7));
        assert!(moved.has(FLAG_FALLING), "他のフラグは保つ");
        assert!(moved.has(FLAG_PARITY) && nb.get(63, 10).has(FLAG_PARITY));
    }

    #[test]
    fn writes_wake_across_chunk_borders() {
        let m = MaterialTable::builtin().unwrap();
        let mut nb = full(&m, 0);
        nb.set(0, 0, cell(7));
        let chunks = nb.into_chunks();
        assert_eq!(
            chunks[4].as_ref().unwrap().next_dirty,
            Some(Rect {
                min_x: 0,
                min_y: 0,
                max_x: 2,
                max_y: 2
            })
        );
        assert_eq!(
            chunks[0].as_ref().unwrap().next_dirty,
            Some(Rect {
                min_x: 62,
                min_y: 62,
                max_x: 63,
                max_y: 63
            })
        );
        assert!(chunks[1].as_ref().unwrap().next_dirty.is_some());
        assert!(chunks[3].as_ref().unwrap().next_dirty.is_some());
        assert!(chunks[8].as_ref().unwrap().next_dirty.is_none());
    }

    #[test]
    fn mark_parity_does_not_wake() {
        let m = MaterialTable::builtin().unwrap();
        let mut nb = full(&m, 1);
        nb.mark_parity(5, 5);
        assert!(nb.get(5, 5).has(FLAG_PARITY));
        let chunks = nb.into_chunks();
        assert!(chunks[4].as_ref().unwrap().next_dirty.is_none());
        assert!(chunks[4].as_ref().unwrap().render_dirty.is_none());
    }

    #[test]
    fn slot_positions() {
        let c = ChunkPos::new(10, -3);
        assert_eq!(Neighborhood::slot_pos(c, 0), ChunkPos::new(9, -4));
        assert_eq!(Neighborhood::slot_pos(c, 4), c);
        assert_eq!(Neighborhood::slot_pos(c, 8), ChunkPos::new(11, -2));
    }
}
