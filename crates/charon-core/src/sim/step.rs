//! 1 tick の流れ (仕様 02 §1〜§4)。

use std::sync::Arc;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

use crate::cell::{Cell, FLAG_BODY, FLAG_BURNING, FLAG_FALLING, FLAG_PARITY, MaterialId};
use crate::coords::{CellPos, ChunkPos};
use crate::material::{MaterialTable, Phase};
use crate::rng::{SALT_FIRE, SALT_FIRE_DIR, SALT_REACT, SALT_REACT_DIR, chance};
use crate::sim::Neighborhood;
use crate::sim::movement::{move_fluid, move_powder};
use crate::world::World;

/// チャンクのフェーズ (0..9)。同じフェーズのチャンクは 3 チャンクずつ離れている。
pub fn chunk_phase(pos: ChunkPos) -> u8 {
    (pos.x.rem_euclid(3) + 3 * pos.y.rem_euclid(3)) as u8
}

impl World {
    /// 1 tick 進める。`parallel` feature が有効なら、同じフェーズのチャンクを並列に処理する。
    pub fn step(&mut self) {
        self.step_with_mode(cfg!(feature = "parallel"));
    }

    /// 並列実装と比較するための逐次更新。両方とも同じ更新関数を使う。
    pub fn step_serial(&mut self) {
        self.step_with_mode(false);
    }

    fn step_with_mode(&mut self, parallel: bool) {
        self.last_tick_cells = 0;
        let mut any_dirty = false;
        let active = self.active;
        for (pos, chunk) in &mut self.chunks {
            // 範囲の外のチャンクは、保留した変化 (`next_dirty`) をそのまま残す。
            let inside =
                active.is_none_or(|(c, r)| (pos.x - c.x).abs() <= r && (pos.y - c.y).abs() <= r);
            chunk.dirty = if inside { chunk.next_dirty.take() } else { None };
            any_dirty |= chunk.dirty.is_some();
        }

        if any_dirty {
            let materials = Arc::clone(&self.materials);
            for phase in 0..9u8 {
                self.last_tick_cells += self.step_phase(phase, &materials, parallel);
            }
        }

        self.tick += 1;
    }

    /// 1 フェーズ分を処理する (仕様 02 §2)。
    fn step_phase(&mut self, phase: u8, materials: &MaterialTable, parallel: bool) -> usize {
        let mut centers: Vec<ChunkPos> = self
            .chunks
            .iter()
            .filter(|(pos, chunk)| chunk.dirty.is_some() && chunk_phase(**pos) == phase)
            .map(|(pos, _)| *pos)
            .collect();
        if centers.is_empty() {
            return 0;
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
                Neighborhood::new(
                    center,
                    chunks,
                    materials,
                    self.reactions.as_deref(),
                    self.seed,
                    self.tick,
                )
            })
            .collect();

        #[cfg(feature = "parallel")]
        let processed: usize = if parallel {
            neighborhoods.par_iter_mut().map(update_chunk).sum()
        } else {
            neighborhoods.iter_mut().map(update_chunk).sum()
        };
        #[cfg(not(feature = "parallel"))]
        let processed: usize = {
            let _ = parallel;
            neighborhoods.iter_mut().map(update_chunk).sum()
        };

        for nb in neighborhoods {
            let center = nb.center;
            for (slot, chunk) in nb.into_chunks().into_iter().enumerate() {
                if let Some(chunk) = chunk {
                    self.chunks.insert(Neighborhood::slot_pos(center, slot), chunk);
                }
            }
        }
        processed
    }
}

/// 中心チャンクの `dirty` の範囲を走査する (仕様 02 §3)。
fn update_chunk(nb: &mut Neighborhood<'_>) -> usize {
    let Some(rect) = nb.take_center_dirty() else {
        return 0;
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
    (usize::from(rect.max_x - rect.min_x) + 1) * (usize::from(rect.max_y - rect.min_y) + 1)
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

/// 材質を変える共通処理 (仕様 03 §1)。温度と色だけを引き継ぎ、寿命を引き直す。
pub fn convert(
    nb: &Neighborhood<'_>,
    x: i32,
    y: i32,
    cell: Cell,
    new_material: MaterialId,
) -> Cell {
    let origin = nb.center.origin();
    convert_cell(
        nb.materials(),
        nb.seed(),
        nb.tick(),
        CellPos::new(origin.x + x, origin.y + y),
        cell,
        new_material,
    )
}

/// 通常の反応と tick 開始時の規則で同じ材質変化を使う。
pub(crate) fn convert_cell(
    materials: &MaterialTable,
    seed: u64,
    tick: u64,
    pos: CellPos,
    cell: Cell,
    new_material: MaterialId,
) -> Cell {
    Cell {
        material: new_material,
        flags: (cell.flags & FLAG_PARITY) | FLAG_FALLING,
        shade: cell.shade,
        temp: cell.temp,
        vy: 0,
        life: crate::world::roll_life(materials.get(new_material).lifetime, seed, tick, pos),
    }
}

// ---- 仕様 03: 寿命 → 熱 → 反応 → 燃焼 ----

fn step_lifetime(nb: &mut Neighborhood<'_>, x: i32, y: i32) {
    let mut cell = nb.get(x, y);
    if cell.life == 0 || cell.has(FLAG_BURNING) {
        return;
    }
    nb.keep_awake(x, y);
    if nb.tick() & 3 != 0 {
        return;
    }
    cell.life -= 1;
    if cell.life == 0 {
        let into = nb.materials().get(cell.material).decay_into;
        cell = convert(nb, x, y, cell, into);
    }
    nb.set(x, y, cell);
}
fn step_heat(nb: &mut Neighborhood<'_>, x: i32, y: i32) {
    for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
        let mut cell = nb.get(x, y);
        let mat = nb.materials().get(cell.material);
        let mut other = nb.get(x + dx, y + dy);
        let om = nb.materials().get(other.material);
        if other.material == MaterialId::BOUNDARY || other.has(FLAG_BODY) {
            continue;
        }
        // 空セルは自分から更新しないため、空との組だけ左・上も扱う。
        if (dx < 0 || dy < 0) && om.phase != Phase::Empty {
            continue;
        }
        let k = mat.conductivity.min(om.conductivity) as i32;
        let transfer = (other.temp as i32 - cell.temp as i32) * k / 1024;
        if transfer == 0 {
            continue;
        }
        // 両方の i16 を守りながら、温度の総和を必ず保存する。
        let low = (i16::MIN as i32 - cell.temp as i32).max(other.temp as i32 - i16::MAX as i32);
        let high = (i16::MAX as i32 - cell.temp as i32).min(other.temp as i32 - i16::MIN as i32);
        let transfer = transfer.clamp(low, high);
        if transfer == 0 {
            continue;
        }
        cell.temp = (cell.temp as i32 + transfer) as i16;
        other.temp = (other.temp as i32 - transfer) as i16;
        nb.set(x, y, cell);
        nb.set(x + dx, y + dy, other);
    }

    let cell = nb.get(x, y);
    let mat = nb.materials().get(cell.material);
    let into = if let Some(t) = mat.boil.filter(|t| cell.temp >= t.at) {
        Some(t.into)
    } else if let Some(t) = mat.melt.filter(|t| cell.temp >= t.at) {
        Some(t.into)
    } else {
        mat.freeze.filter(|t| cell.temp <= t.at).map(|t| t.into)
    };
    if let Some(into) = into {
        nb.set(x, y, convert(nb, x, y, cell, into));
    }
}
fn step_reaction(nb: &mut Neighborhood<'_>, x: i32, y: i32) {
    let Some(reactions) = nb.reactions() else { return };
    let cell = nb.get(x, y);
    if !reactions.is_reactive(cell.material) {
        return;
    }
    const DIRS: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
    if !DIRS.iter().any(|&(dx, dy)| {
        let other = nb.get(x + dx, y + dy);
        !other.has(FLAG_BODY) && reactions.get(cell.material, other.material).is_some()
    }) {
        return;
    }
    nb.keep_awake(x, y);
    let (dx, dy) = DIRS[(nb.hash(x, y, SALT_REACT_DIR) & 3) as usize];
    let other = nb.get(x + dx, y + dy);
    if other.has(FLAG_BODY) {
        return;
    }
    let Some(reaction) = reactions.get(cell.material, other.material) else { return };
    if !chance(nb.hash(x, y, SALT_REACT), reaction.prob) {
        return;
    }
    nb.set(x, y, convert(nb, x, y, cell, reaction.out_a));
    nb.set(x + dx, y + dy, convert(nb, x + dx, y + dy, other, reaction.out_b));
}
fn step_burning(nb: &mut Neighborhood<'_>, x: i32, y: i32) {
    let mut cell = nb.get(x, y);
    let mat = nb.materials().get(cell.material);
    if !cell.has(FLAG_BURNING) {
        if mat.ignite_at.is_some_and(|at| cell.temp >= at) {
            cell.flags |= FLAG_BURNING;
            cell.life = mat.burn_time.max(1);
            nb.set(x, y, cell);
        }
        return;
    }

    nb.keep_awake(x, y);
    let difference = (mat.burn_temp as i32 - cell.temp as i32).clamp(-50, 50);
    cell.temp = (cell.temp as i32 + difference) as i16;
    let dirs = [(0, -1), (1, 0), (0, 1), (-1, 0)];
    let (dx, dy) = dirs[(nb.hash(x, y, SALT_FIRE_DIR) & 3) as usize];
    let target = nb.get(x + dx, y + dy);
    if target.material == MaterialId::EMPTY
        && !target.has(FLAG_BODY)
        && chance(nb.hash(x, y, SALT_FIRE), 64)
        && let Some(fire) = nb.materials().fire
    {
        nb.set(x + dx, y + dy, nb.make_cell(fire, x + dx, y + dy));
    }
    if nb.tick() & 3 == 0 {
        cell.life -= 1;
    }
    if cell.life == 0 {
        cell = convert(nb, x, y, cell, mat.burn_into);
    }
    if cell != nb.get(x, y) {
        nb.set(x, y, cell);
    }
}

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
    fn convert_preserves_temp_and_shade_and_rerolls_life() {
        let m = MaterialTable::builtin().unwrap();
        let nb = Neighborhood::new(
            ChunkPos::new(1, -1),
            std::array::from_fn(|_| Some(Box::new(crate::chunk::Chunk::new()))),
            &m,
            None,
            42,
            9,
        );
        let original = Cell {
            material: m.id("water").unwrap(),
            flags: FLAG_PARITY | crate::cell::FLAG_BURNING | crate::cell::FLAG_FLOW_RIGHT,
            shade: 137,
            temp: 456,
            vy: 22,
            life: 99,
        };
        let smoke = m.id("smoke").unwrap();
        let changed = convert(&nb, 3, 4, original, smoke);
        assert_eq!(changed.material, smoke);
        assert_eq!((changed.temp, changed.shade, changed.vy), (456, 137, 0));
        assert_eq!(changed.flags, FLAG_PARITY | FLAG_FALLING);
        assert!((30..=90).contains(&changed.life));
        assert_eq!(changed, convert(&nb, 3, 4, original, smoke));

        let stone = convert(&nb, 3, 4, original, m.id("stone").unwrap());
        assert_eq!(stone.life, 0);
        assert_eq!(stone.flags, FLAG_PARITY | FLAG_FALLING);
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
    fn sleeping_10000_chunks_skip_all_phases() {
        let mut world = World::new(Arc::new(MaterialTable::builtin().unwrap()), 1);
        for y in 0..100 {
            for x in 0..100 {
                world.insert_chunk(ChunkPos::new(x, y));
            }
        }
        assert_eq!(world.awake_chunks(), 0);
        world.step();
        assert_eq!(world.tick(), 1);
        assert_eq!(world.awake_chunks(), 0);
        assert_eq!(world.last_tick_cells(), 0);
    }

    #[test]
    fn last_tick_cells_counts_scanned_area_and_resets() {
        let mut world = world_3x3();
        assert_eq!(world.last_tick_cells(), 0);
        world.keep_awake(CellPos::new(10, 10));
        world.step();
        assert_eq!(world.last_tick_cells(), 25);
        assert_eq!(world.awake_chunks(), 0);
        world.step();
        assert_eq!(world.last_tick_cells(), 0);
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
