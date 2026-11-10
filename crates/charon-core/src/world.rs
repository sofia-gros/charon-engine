//! ワールド (仕様 01 §5、§6)。

use std::collections::HashMap;
use std::sync::Arc;

use crate::cell::{Cell, FLAG_FALLING, FLAG_FLOW_RIGHT, FLAG_PARITY, MaterialId};
use crate::chunk::{Chunk, add_rect, wake_region};
use crate::coords::{CHUNK_MASK, CellPos, ChunkPos};
use crate::material::MaterialTable;
use crate::rng::{SALT_FLOW, SALT_LIFE, SALT_SHADE, hash};
use crate::sim::SCAN;

/// 読み込まれているチャンクの集まり。
#[derive(Debug)]
pub struct World {
    pub(crate) chunks: HashMap<ChunkPos, Box<Chunk>>,
    pub(crate) materials: Arc<MaterialTable>,
    pub(crate) seed: u64,
    pub(crate) tick: u64,
}

impl World {
    /// 空のワールド。
    pub fn new(materials: Arc<MaterialTable>, seed: u64) -> Self {
        Self { chunks: HashMap::new(), materials, seed, tick: 0 }
    }

    /// 全セルが空のチャンクを追加する。既にあれば何もしない。
    pub fn insert_chunk(&mut self, pos: ChunkPos) {
        self.chunks.entry(pos).or_insert_with(|| Box::new(Chunk::new()));
    }

    /// セルを読む。チャンクがなければ `BOUNDARY` のセルを返す。
    pub fn get(&self, pos: CellPos) -> Cell {
        match self.chunks.get(&pos.chunk()) {
            Some(chunk) => chunk.get(local(pos.x), local(pos.y)),
            None => Cell::BOUNDARY,
        }
    }

    /// セルを書き込み、仕様 01 §6 の処理を行う。チャンクがなければ何もしない。
    ///
    /// 材質が変わった場合は、その場所を行き先として見られる液体・気体が気づけるよう、
    /// 上下 1 行・左右 `SCAN + 1` セルの帯も起こす (仕様 02 §9)。外からの編集は頻繁ではないので、
    /// シミュレーション中の書き込み (`Neighborhood::set`) と違い、相手を探さずに帯ごと起こす。
    pub fn set(&mut self, pos: CellPos, cell: Cell) {
        let cpos = pos.chunk();
        let Some(chunk) = self.chunks.get_mut(&cpos) else { return };
        let (lx, ly) = (local(pos.x), local(pos.y));
        let changed = chunk.get(lx, ly).material != cell.material;
        chunk.write(lx, ly, cell, &self.materials);
        self.keep_awake(pos);
        if changed {
            for dy in -1..=1 {
                // keep_awake は左右 2 セルを起こすので、4 セルおきで帯が埋まる。
                for dx in (-(SCAN + 1)..=SCAN + 1).step_by(4) {
                    self.keep_awake(CellPos::new(pos.x + dx, pos.y + dy));
                }
            }
        }
    }

    /// セルの値は変えずに、周囲 5×5 を次の tick の処理範囲に加える。
    pub fn keep_awake(&mut self, pos: CellPos) {
        let cpos = pos.chunk();
        wake_region(pos.x & CHUNK_MASK, pos.y & CHUNK_MASK, |dcx, dcy, rect| {
            if let Some(c) = self.chunks.get_mut(&ChunkPos::new(cpos.x + dcx, cpos.y + dcy)) {
                add_rect(&mut c.next_dirty, rect);
            }
        });
    }

    /// 新しいセルを作る (置いた直後は動ける状態)。
    pub fn make_cell(&self, material: MaterialId, pos: CellPos) -> Cell {
        make_cell(&self.materials, self.seed, self.tick, material, pos)
    }

    /// 現在の tick。
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// ワールドの seed。
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// 材質の表。
    pub fn materials(&self) -> &Arc<MaterialTable> {
        &self.materials
    }

    /// 読み込まれているチャンクを直接見る (描画・保存用)。
    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos).map(|c| &**c)
    }

    /// `dirty` または `next_dirty` が `Some` のチャンクの数。0 なら世界全体が休眠している。
    pub fn awake_chunks(&self) -> usize {
        self.chunks.values().filter(|c| c.dirty.is_some() || c.next_dirty.is_some()).count()
    }

    /// 読み込まれているチャンクの位置。昇順。
    pub fn chunk_positions(&self) -> Vec<ChunkPos> {
        let mut v: Vec<ChunkPos> = self.chunks.keys().copied().collect();
        v.sort_unstable();
        v
    }

    /// 全セルのハッシュ (FNV-1a 64)。`FLAG_PARITY` は無視する。決定性の確認に使う。
    pub fn state_hash(&self) -> u64 {
        const PRIME: u64 = 0x0000_0100_0000_01B3;
        let mut h: u64 = 0xCBF2_9CE4_8422_2325;
        let mut mix = |b: u8| {
            h ^= b as u64;
            h = h.wrapping_mul(PRIME);
        };
        for pos in self.chunk_positions() {
            let Some(chunk) = self.chunks.get(&pos) else {
                continue;
            };
            for c in chunk.cells.iter() {
                let [m0, m1] = c.material.0.to_le_bytes();
                let [t0, t1] = c.temp.to_le_bytes();
                for b in [m0, m1, c.flags & !FLAG_PARITY, c.shade, t0, t1, c.vy as u8, c.life] {
                    mix(b);
                }
            }
        }
        h
    }
}

fn local(v: i32) -> u8 {
    (v & CHUNK_MASK) as u8
}

/// 新しいセルを作る共通の式 (仕様 01 §5)。
pub(crate) fn make_cell(
    materials: &MaterialTable,
    seed: u64,
    tick: u64,
    material: MaterialId,
    pos: CellPos,
) -> Cell {
    let mat = materials.get(material);
    let mut flags = FLAG_FALLING;
    if hash(seed, tick, pos.x, pos.y, SALT_FLOW) & 1 == 1 {
        flags |= FLAG_FLOW_RIGHT;
    }
    Cell {
        material,
        flags,
        shade: hash(seed, tick, pos.x, pos.y, SALT_SHADE) as u8,
        temp: mat.initial_temp,
        vy: 0,
        life: roll_life(mat.lifetime, seed, tick, pos),
    }
}

/// 材質の `lifetime` から寿命を引く。`None` なら 0 (寿命なし)。
pub(crate) fn roll_life(lifetime: Option<(u8, u8)>, seed: u64, tick: u64, pos: CellPos) -> u8 {
    match lifetime {
        Some((min, max)) if max >= min => {
            let span = (max - min) as u32 + 1;
            min + (hash(seed, tick, pos.x, pos.y, SALT_LIFE) % span) as u8
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Rect;

    fn world() -> World {
        World::new(Arc::new(MaterialTable::builtin().unwrap()), 1)
    }

    fn id(w: &World, name: &str) -> MaterialId {
        w.materials.id(name).unwrap()
    }

    #[test]
    fn missing_chunk_reads_boundary_and_ignores_writes() {
        let mut w = world();
        let p = CellPos::new(5, 5);
        assert_eq!(w.get(p).material, MaterialId::BOUNDARY);
        let sand = w.make_cell(id(&w, "sand"), p);
        w.set(p, sand);
        assert_eq!(w.get(p).material, MaterialId::BOUNDARY);
    }

    #[test]
    fn set_get_roundtrip_with_negative_coords() {
        let mut w = world();
        w.insert_chunk(ChunkPos::new(-1, -1));
        let p = CellPos::new(-1, -64);
        let sand = w.make_cell(id(&w, "sand"), p);
        w.set(p, sand);
        assert_eq!(w.get(p), sand);
        assert_eq!(w.get(CellPos::new(-2, -64)), Cell::default());
    }

    #[test]
    fn keep_awake_marks_5x5() {
        let mut w = world();
        w.insert_chunk(ChunkPos::new(0, 0));
        w.keep_awake(CellPos::new(10, 20));
        let c = w.chunk(ChunkPos::new(0, 0)).unwrap();
        assert_eq!(c.next_dirty, Some(Rect { min_x: 8, min_y: 18, max_x: 12, max_y: 22 }));
        assert_eq!(c.render_dirty, None);
        assert_eq!(w.awake_chunks(), 1);
    }

    #[test]
    fn keep_awake_at_border_wakes_neighbours() {
        let mut w = world();
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            w.insert_chunk(ChunkPos::new(x, y));
        }
        w.keep_awake(CellPos::new(63, 63));
        assert_eq!(w.awake_chunks(), 4);
        let right = w.chunk(ChunkPos::new(1, 0)).unwrap();
        assert_eq!(right.next_dirty, Some(Rect { min_x: 0, min_y: 61, max_x: 1, max_y: 63 }));
    }

    #[test]
    fn set_marks_render_dirty_and_a_wide_strip() {
        let mut w = world();
        w.insert_chunk(ChunkPos::new(0, 0));
        let p = CellPos::new(40, 20);
        let water = w.make_cell(id(&w, "water"), p);
        w.set(p, water);
        let c = w.chunk(ChunkPos::new(0, 0)).unwrap();
        assert_eq!(c.render_dirty, Some(Rect::point(40, 20)), "変わったセルだけを再送する");
        assert!(!c.collider_dirty, "液体は当たり判定に関係しない");
        // 材質が変わったので、左右 SCAN + 1 セル・上下 1 行 (+ 余白 2) を起こす
        let d = c.next_dirty.unwrap();
        assert!(d.min_x <= 40 - 25 && d.max_x == 63);
        assert_eq!((d.min_y, d.max_y), (17, 23));

        // 材質が変わらない書き込みは 5×5 だけ
        let mut w = world();
        w.insert_chunk(ChunkPos::new(0, 0));
        w.set(p, Cell::default());
        let c = w.chunk(ChunkPos::new(0, 0)).unwrap();
        assert_eq!(c.next_dirty, Some(Rect { min_x: 38, min_y: 18, max_x: 42, max_y: 22 }));
    }

    #[test]
    fn collider_dirty_for_solids_and_resting_powder() {
        let mut w = world();
        w.insert_chunk(ChunkPos::new(0, 0));
        let p = CellPos::new(1, 1);
        // 動いている粉体は当たり判定に含めない
        let sand = w.make_cell(id(&w, "sand"), p);
        w.set(p, sand);
        assert!(!w.chunk(ChunkPos::new(0, 0)).unwrap().collider_dirty);
        // 止まった粉体は含める
        w.set(p, sand.with(FLAG_FALLING, false));
        assert!(w.chunk(ChunkPos::new(0, 0)).unwrap().collider_dirty);
    }

    #[test]
    fn make_cell_uses_material_defaults() {
        let w = world();
        let p = CellPos::new(3, 4);
        let lava = w.make_cell(id(&w, "lava"), p);
        assert_eq!(lava.temp, 1400);
        assert!(lava.has(FLAG_FALLING));
        assert_eq!((lava.vy, lava.life), (0, 0));
        let smoke = w.make_cell(id(&w, "smoke"), p);
        assert!((30..=90).contains(&smoke.life));
        assert_eq!(w.make_cell(id(&w, "lava"), p), lava, "同じ入力なら同じセル");
    }

    #[test]
    fn state_hash_ignores_parity_only() {
        let mut w = world();
        w.insert_chunk(ChunkPos::new(0, 0));
        let p = CellPos::new(1, 1);
        let sand = w.make_cell(id(&w, "sand"), p);
        w.set(p, sand);
        let h = w.state_hash();
        w.set(p, sand.with(FLAG_PARITY, true));
        assert_eq!(w.state_hash(), h);
        w.set(p, sand.with(FLAG_FALLING, false));
        assert_ne!(w.state_hash(), h);
    }

    #[test]
    fn chunk_positions_are_sorted() {
        let mut w = world();
        for (x, y) in [(2, 0), (-1, 5), (0, 0), (-1, -3)] {
            w.insert_chunk(ChunkPos::new(x, y));
        }
        let v = w.chunk_positions();
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(v, sorted);
        assert_eq!(v.len(), 4);
    }
}
