//! 1 tick の流れ (仕様 02 §1〜§4)。

use std::sync::Arc;

use crate::cell::FLAG_BODY;
use crate::coords::ChunkPos;
use crate::material::{MaterialTable, Phase};
use crate::sim::Neighborhood;
use crate::sim::movement::{move_fluid, move_powder};
use crate::world::World;

/// チャンクのフェーズ (0..9)。同じフェーズのチャンクは 3 チャンクずつ離れている。
pub fn chunk_phase(pos: ChunkPos) -> u8 {
    (pos.x.rem_euclid(3) + 3 * pos.y.rem_euclid(3)) as u8
}

impl World {
    /// 1 tick 進める。
    pub fn step(&mut self) {
        for chunk in self.chunks.values_mut() {
            chunk.dirty = chunk.next_dirty.take();
        }

        let materials = Arc::clone(&self.materials);
        for phase in 0..9u8 {
            self.step_phase(phase, &materials);
        }

        self.tick += 1;
    }

    /// 1 フェーズ分を処理する (仕様 02 §2)。
    fn step_phase(&mut self, phase: u8, materials: &MaterialTable) {
        let mut centers: Vec<ChunkPos> = self
            .chunks
            .iter()
            .filter(|(pos, chunk)| chunk.dirty.is_some() && chunk_phase(**pos) == phase)
            .map(|(pos, _)| *pos)
            .collect();
        if centers.is_empty() {
            return;
        }
        // HashMap の反復順を結果に影響させない。
        centers.sort_unstable();

        // 同じフェーズの中心は 3 チャンクずつ離れているので、取り出す 9 チャンクは重ならない。
        let mut neighborhoods: Vec<Neighborhood<'_>> = centers
            .iter()
            .map(|&center| {
                let chunks = std::array::from_fn(|slot| {
                    self.chunks.remove(&Neighborhood::slot_pos(center, slot))
                });
                Neighborhood::new(center, chunks, materials, self.seed, self.tick)
            })
            .collect();

        for nb in &mut neighborhoods {
            update_chunk(nb);
        }

        for nb in neighborhoods {
            let center = nb.center;
            for (slot, chunk) in nb.into_chunks().into_iter().enumerate() {
                if let Some(chunk) = chunk {
                    self.chunks.insert(Neighborhood::slot_pos(center, slot), chunk);
                }
            }
        }
    }
}

/// 中心チャンクの `dirty` の範囲を走査する (仕様 02 §3)。
fn update_chunk(nb: &mut Neighborhood<'_>) {
    let Some(rect) = nb.take_center_dirty() else {
        return;
    };
    // 行は下から上。
    for y in (rect.min_y as i32..=rect.max_y as i32).rev() {
        let left_to_right = nb.row_hash(y) & 1 == 0;
        let (min_x, max_x) = (rect.min_x as i32, rect.max_x as i32);
        for i in 0..=(max_x - min_x) {
            let x = if left_to_right { min_x + i } else { max_x - i };
            update_cell(nb, x, y);
        }
    }
}

/// 1 セルを更新する (仕様 02 §4)。
fn update_cell(nb: &mut Neighborhood<'_>, x: i32, y: i32) {
    let cell = nb.get(x, y);
    if cell.has(FLAG_BODY) {
        return;
    }
    let mat = nb.materials().get(cell.material);
    if mat.phase == Phase::Empty {
        return;
    }
    let movable = matches!(mat.phase, Phase::Powder | Phase::Liquid | Phase::Gas);
    if movable && cell.has(crate::cell::FLAG_PARITY) == nb.parity() {
        // この tick で既に動いたセルか、眠っている間に parity が古くなったセル。
        // 後者を取りこぼさないよう、次の tick まで起こしておく (仕様 02 §3)。
        nb.keep_awake(x, y);
        return;
    }

    step_lifetime(nb, x, y);
    step_heat(nb, x, y);
    step_reaction(nb, x, y);
    step_burning(nb, x, y);

    // step_* が材質を変えることがあるので読み直す。
    let cell = nb.get(x, y);
    let mat = nb.materials().get(cell.material);
    match mat.phase {
        Phase::Powder => {
            nb.mark_parity(x, y);
            move_powder(nb, x, y, cell, mat);
        }
        Phase::Liquid | Phase::Gas => {
            nb.mark_parity(x, y);
            move_fluid(nb, x, y, cell, mat);
        }
        Phase::Solid | Phase::Empty => {}
    }
}

// ---- M2 で実装する (仕様 03) ----

fn step_lifetime(_nb: &mut Neighborhood<'_>, _x: i32, _y: i32) {}
fn step_heat(_nb: &mut Neighborhood<'_>, _x: i32, _y: i32) {}
fn step_reaction(_nb: &mut Neighborhood<'_>, _x: i32, _y: i32) {}
fn step_burning(_nb: &mut Neighborhood<'_>, _x: i32, _y: i32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::FLAG_PARITY;
    use crate::coords::CellPos;

    fn world_3x3() -> World {
        let mut w = World::new(Arc::new(MaterialTable::builtin().unwrap()), 1);
        for y in -1..=1 {
            for x in -1..=1 {
                w.insert_chunk(ChunkPos::new(x, y));
            }
        }
        w
    }

    #[test]
    fn phases_are_three_apart() {
        for (x, y) in [(0, 0), (-1, -1), (5, -7), (-4, 2)] {
            let p = chunk_phase(ChunkPos::new(x, y));
            assert!(p < 9);
            assert_eq!(p, chunk_phase(ChunkPos::new(x + 3, y)));
            assert_eq!(p, chunk_phase(ChunkPos::new(x, y - 3)));
            assert_ne!(p, chunk_phase(ChunkPos::new(x + 1, y)));
            assert_ne!(p, chunk_phase(ChunkPos::new(x, y + 2)));
        }
    }

    #[test]
    fn step_advances_tick_and_keeps_chunks() {
        let mut w = world_3x3();
        let p = CellPos::new(-1, 64);
        let stone = w.make_cell(w.materials().id("stone").unwrap(), p);
        w.set(p, stone);
        let before = w.state_hash();
        w.step();
        assert_eq!(w.tick(), 1);
        assert_eq!(w.chunk_positions().len(), 9);
        assert_eq!(w.state_hash(), before);
        assert_eq!(w.get(p).material, stone.material);
    }

    #[test]
    fn untouched_world_goes_to_sleep() {
        let mut w = world_3x3();
        let p = CellPos::new(0, 0); // 4 つのチャンクの角
        let stone = w.make_cell(w.materials().id("stone").unwrap(), p);
        w.set(p, stone);
        assert_eq!(w.awake_chunks(), 4);
        w.step();
        assert_eq!(w.awake_chunks(), 0);
    }

    #[test]
    fn stale_parity_delays_by_one_tick_instead_of_being_lost() {
        let mut w = world_3x3();
        let p = CellPos::new(5, 5);
        // tick 0 の parity (false) と同じ値を持つ、動かせる材質のセル
        let sand = w.make_cell(w.materials().id("sand").unwrap(), p).with(FLAG_PARITY, false);
        w.set(p, sand);
        w.step();
        assert_eq!(w.awake_chunks(), 1, "飛ばしたセルは次の tick まで起こしておく");
        w.step();
        assert!(w.get(p).has(FLAG_PARITY), "tick 1 で処理され、parity が更新された");
    }
}
