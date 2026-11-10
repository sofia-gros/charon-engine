//! 座標 (仕様 01 §1)。
//!
//! 単位はセル。x は右が正、y は下が正。

/// チャンクの一辺を表すビット数。
pub const CHUNK_SHIFT: u32 = 6;
/// チャンクの一辺のセル数 (64)。
pub const CHUNK_SIZE: i32 = 1 << CHUNK_SHIFT;
/// チャンク内座標を取り出すマスク (63)。
pub const CHUNK_MASK: i32 = CHUNK_SIZE - 1;
/// 1 チャンクのセル数 (4096)。
pub const CHUNK_AREA: usize = (CHUNK_SIZE * CHUNK_SIZE) as usize;

/// ワールド上のセルの位置。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct CellPos {
    pub x: i32,
    pub y: i32,
}

/// チャンクの位置 (チャンク単位)。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ChunkPos {
    pub x: i32,
    pub y: i32,
}

impl CellPos {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// このセルを含むチャンク。
    ///
    /// 算術右シフトは負数でも床方向に丸める。割り算を使ってはいけない。
    pub const fn chunk(self) -> ChunkPos {
        ChunkPos { x: self.x >> CHUNK_SHIFT, y: self.y >> CHUNK_SHIFT }
    }

    /// チャンク内の添字 (0..4096)。行優先: `ly * 64 + lx`。
    pub const fn local_index(self) -> usize {
        ((self.y & CHUNK_MASK) * CHUNK_SIZE + (self.x & CHUNK_MASK)) as usize
    }
}

impl ChunkPos {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// このチャンクの左上のセル。
    pub const fn origin(self) -> CellPos {
        CellPos { x: self.x << CHUNK_SHIFT, y: self.y << CHUNK_SHIFT }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_cell_maps_to_negative_chunk() {
        let p = CellPos::new(-1, -1);
        assert_eq!(p.chunk(), ChunkPos::new(-1, -1));
        assert_eq!(p.local_index(), 4095);
    }

    #[test]
    fn chunk_boundary() {
        let p = CellPos::new(64, 0);
        assert_eq!(p.chunk(), ChunkPos::new(1, 0));
        assert_eq!(p.local_index(), 0);
    }

    #[test]
    fn local_index_is_row_major() {
        assert_eq!(CellPos::new(3, 2).local_index(), 2 * 64 + 3);
        assert_eq!(CellPos::new(-64, 65).local_index(), 64);
    }

    #[test]
    fn origin_roundtrip() {
        for &(x, y) in &[(0, 0), (-1, 3), (5, -7)] {
            let c = ChunkPos::new(x, y);
            assert_eq!(c.origin().chunk(), c);
            assert_eq!(c.origin().local_index(), 0);
        }
    }
}
