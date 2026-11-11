//! チャンクの GPU テクスチャ配列レイヤー割り当て (仕様 06 §3)。

use std::collections::HashMap;

use charon_core::coords::ChunkPos;

/// 使われていない層をスタックで再利用する。
#[derive(Debug)]
pub struct LayerAllocator {
    occupied: HashMap<ChunkPos, u32>,
    free: Vec<u32>,
    next: u32,
    capacity: u32,
}

impl LayerAllocator {
    pub fn new(capacity: u32) -> Self {
        Self { occupied: HashMap::new(), free: Vec::new(), next: 0, capacity }
    }

    /// 既存の割り当て、空き層、未使用の層の順に返す。満杯なら `None`。
    pub fn allocate(&mut self, pos: ChunkPos) -> Option<u32> {
        if let Some(&layer) = self.occupied.get(&pos) {
            return Some(layer);
        }
        let layer = if let Some(layer) = self.free.pop() {
            layer
        } else if self.next < self.capacity {
            let layer = self.next;
            self.next += 1;
            layer
        } else {
            return None;
        };
        self.occupied.insert(pos, layer);
        Some(layer)
    }

    /// チャンクの層を解放する。次の `allocate` が再利用する。
    pub fn release(&mut self, pos: ChunkPos) -> Option<u32> {
        let layer = self.occupied.remove(&pos)?;
        self.free.push(layer);
        Some(layer)
    }

    pub fn layer(&self, pos: ChunkPos) -> Option<u32> {
        self.occupied.get(&pos).copied()
    }

    /// 割り当て済みチャンクの位置を昇順で返す。
    pub fn positions(&self) -> Vec<ChunkPos> {
        let mut positions: Vec<_> = self.occupied.keys().copied().collect();
        positions.sort_unstable();
        positions
    }

    pub fn len(&self) -> usize {
        self.occupied.len()
    }

    pub fn is_empty(&self) -> bool {
        self.occupied.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_allocator() {
        let mut layers = LayerAllocator::new(2);
        let a = ChunkPos::new(0, 0);
        let b = ChunkPos::new(1, 0);
        let c = ChunkPos::new(2, 0);
        assert_eq!(layers.allocate(a), Some(0));
        assert_eq!(layers.allocate(a), Some(0));
        assert_eq!(layers.allocate(b), Some(1));
        assert_eq!(layers.allocate(c), None);
        assert_ne!(layers.layer(a), layers.layer(b));
        assert_eq!(layers.release(a), Some(0));
        assert_eq!(layers.release(a), None);
        assert_eq!(layers.allocate(c), Some(0));
        assert_eq!(layers.layer(b), Some(1));
        assert_eq!(layers.len(), 2);
    }
}
