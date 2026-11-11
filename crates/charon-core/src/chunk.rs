//! チャンク (仕様 01 §5、§6)。

use crate::cell::{Cell, FLAG_BODY, FLAG_FALLING};
use crate::coords::{CHUNK_AREA, CHUNK_SIZE};
use crate::material::{MaterialTable, Phase};

/// チャンク内座標 (0..64) の矩形。min と max は両端を含む。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    pub min_x: u8,
    pub min_y: u8,
    pub max_x: u8,
    pub max_y: u8,
}

impl Rect {
    /// 1 点だけの矩形。
    pub const fn point(x: u8, y: u8) -> Self {
        Self { min_x: x, min_y: y, max_x: x, max_y: y }
    }

    /// 2 つの矩形を含む最小の矩形。
    #[must_use]
    pub fn union(self, other: Rect) -> Rect {
        Rect {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    pub const fn contains(self, x: u8, y: u8) -> bool {
        self.min_x <= x && x <= self.max_x && self.min_y <= y && y <= self.max_y
    }
}

/// `target` に `rect` を加える (`None` は空の矩形)。
pub fn add_rect(target: &mut Option<Rect>, rect: Rect) {
    *target = Some(match *target {
        Some(cur) => cur.union(rect),
        None => rect,
    });
}

/// 64×64 セルのまとまり。
#[derive(Clone, Debug)]
pub struct Chunk {
    pub cells: Box<[Cell; CHUNK_AREA]>,
    /// この tick で処理する範囲。
    pub dirty: Option<Rect>,
    /// 次の tick で処理する範囲 (書き込みのたびに広げる)。
    pub next_dirty: Option<Rect>,
    /// 固体の形が変わった (仕様 05)。
    pub collider_dirty: bool,
    /// 描画側へ再送が必要な範囲 (仕様 06)。
    pub render_dirty: Option<Rect>,
}

impl Default for Chunk {
    fn default() -> Self {
        Self::new()
    }
}

impl Chunk {
    /// 全セルが空のチャンク。
    pub fn new() -> Self {
        Self {
            cells: Box::new([Cell::default(); CHUNK_AREA]),
            dirty: None,
            next_dirty: None,
            collider_dirty: false,
            render_dirty: None,
        }
    }

    /// チャンク内座標 (0..64) のセルを読む。
    pub fn get(&self, lx: u8, ly: u8) -> Cell {
        self.cells[index(lx, ly)]
    }

    /// セルを書き込み、このチャンク自身に関する記録 (`render_dirty`、`collider_dirty`) を更新する。
    ///
    /// **`next_dirty` は更新しない。** 書き込みの周囲 5×5 は隣のチャンクにまたがるので、
    /// 呼ぶ側が [`wake_region`] を使って該当する全チャンクに付ける (仕様 01 §6)。
    /// 直接 `cells[i] = ...` と書いてよいのはこの関数の中だけである。
    pub(crate) fn write(&mut self, lx: u8, ly: u8, cell: Cell, materials: &MaterialTable) {
        let i = index(lx, ly);
        let old = self.cells[i];
        self.cells[i] = cell;
        add_rect(&mut self.render_dirty, Rect::point(lx, ly));
        if is_collidable(old, materials) || is_collidable(cell, materials) {
            self.collider_dirty = true;
        }
    }

    /// `FLAG_PARITY` だけを書き換える。状態の変化ではないので dirty は付けない (仕様 02 §3)。
    pub(crate) fn write_parity(&mut self, lx: u8, ly: u8, parity: bool) {
        let i = index(lx, ly);
        let c = self.cells[i];
        self.cells[i] = c.with(crate::cell::FLAG_PARITY, parity);
    }
}

fn index(lx: u8, ly: u8) -> usize {
    debug_assert!((lx as i32) < CHUNK_SIZE && (ly as i32) < CHUNK_SIZE);
    (ly as usize % CHUNK_SIZE as usize) * CHUNK_SIZE as usize + (lx as usize % CHUNK_SIZE as usize)
}

/// 地形の当たり判定に含まれるセルか: 固体、または動いていない粉体。
fn is_collidable(cell: Cell, materials: &MaterialTable) -> bool {
    if cell.has(FLAG_BODY) {
        return false;
    }
    match materials.get(cell.material).phase {
        Phase::Solid => true,
        Phase::Powder => !cell.has(FLAG_FALLING),
        _ => false,
    }
}

/// 書き込み位置の周囲 5×5 が、どのチャンクのどの範囲に当たるかを列挙する。
///
/// `(lx, ly)` は、あるチャンクの左上を原点とした座標 (そのチャンクの外でもよい)。
/// `f(dcx, dcy, rect)` は、原点のチャンクから `(dcx, dcy)` だけ離れたチャンクと、
/// そのチャンク内座標での範囲を受け取る。
pub(crate) fn wake_region(lx: i32, ly: i32, mut f: impl FnMut(i32, i32, Rect)) {
    const MARGIN: i32 = 2;
    let (x0, x1, y0, y1) = (lx - MARGIN, lx + MARGIN, ly - MARGIN, ly + MARGIN);
    for dcy in (y0 >> 6)..=(y1 >> 6) {
        for dcx in (x0 >> 6)..=(x1 >> 6) {
            let (ox, oy) = (dcx * CHUNK_SIZE, dcy * CHUNK_SIZE);
            let rect = Rect {
                min_x: (x0.max(ox) - ox) as u8,
                min_y: (y0.max(oy) - oy) as u8,
                max_x: (x1.min(ox + CHUNK_SIZE - 1) - ox) as u8,
                max_y: (y1.min(oy + CHUNK_SIZE - 1) - oy) as u8,
            };
            f(dcx, dcy, rect);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(lx: i32, ly: i32) -> Vec<(i32, i32, Rect)> {
        let mut v = Vec::new();
        wake_region(lx, ly, |dx, dy, r| v.push((dx, dy, r)));
        v
    }

    #[test]
    fn wake_region_inside_one_chunk() {
        assert_eq!(
            collect(10, 20),
            vec![(0, 0, Rect { min_x: 8, min_y: 18, max_x: 12, max_y: 22 })]
        );
    }

    #[test]
    fn wake_region_crosses_corner() {
        let v = collect(0, 63);
        assert_eq!(v.len(), 4);
        assert!(v.contains(&(-1, 0, Rect { min_x: 62, min_y: 61, max_x: 63, max_y: 63 })));
        assert!(v.contains(&(0, 0, Rect { min_x: 0, min_y: 61, max_x: 2, max_y: 63 })));
        assert!(v.contains(&(-1, 1, Rect { min_x: 62, min_y: 0, max_x: 63, max_y: 1 })));
        assert!(v.contains(&(0, 1, Rect { min_x: 0, min_y: 0, max_x: 2, max_y: 1 })));
    }

    #[test]
    fn wake_region_from_outside_origin_chunk() {
        // 原点のチャンクの左隣 (-1) の中だけに収まる
        assert_eq!(
            collect(-10, 5),
            vec![(-1, 0, Rect { min_x: 52, min_y: 3, max_x: 56, max_y: 7 })]
        );
    }

    #[test]
    fn rect_union() {
        let mut r = None;
        add_rect(&mut r, Rect::point(5, 6));
        add_rect(&mut r, Rect::point(2, 9));
        assert_eq!(r, Some(Rect { min_x: 2, min_y: 6, max_x: 5, max_y: 9 }));
    }
}
