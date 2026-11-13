//! 地形の崩落 (仕様 10 §16): 掘って支えを失った固体のかたまりが、ひとかたまりのまま落ちる。
//!
//! セルは入れ替え (`swap`) で動かすので、数は変わらない。材質の名前は使わず、相 (`Phase::Solid`) で判断する。

use std::collections::BTreeSet;

use charon_core::cell::{FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::Phase;
use charon_core::world::World;

/// これより大きいかたまりは「地面とつながっている」とみなして落とさない。
pub const MAX_CLUSTER: usize = 400;
/// 同時に落とすかたまりの数の上限。
pub const MAX_FALLING: usize = 32;

/// 落ちているかたまり。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Cluster {
    /// セルの位置。下の行から順 (y の大きい順、x の小さい順)。
    cells: Vec<CellPos>,
}

/// 崩落の状態。`GameState::collapse` に `Some` を入れると、掘ったあとに働く。
#[derive(Clone, Debug, Default)]
pub struct Collapse {
    falling: Vec<Cluster>,
}

impl Collapse {
    pub fn new() -> Self {
        Self::default()
    }

    /// 落ちているかたまりの数。
    pub fn falling(&self) -> usize {
        self.falling.len()
    }

    /// 落ちているセルの総数。
    pub fn falling_cells(&self) -> usize {
        self.falling.iter().map(|c| c.cells.len()).sum()
    }

    fn solid(world: &World, pos: CellPos) -> Option<bool> {
        let cell = world.get(pos);
        if cell.material == MaterialId::BOUNDARY {
            return None;
        }
        Some(cell.is_solid(world.materials()) && !cell.has(FLAG_BODY))
    }

    /// 掘った位置のまわりの固体のかたまりを調べ、支えのないものを落とし始める。
    /// 支えがある = 大きい (`MAX_CLUSTER` 以上) か、読み込まれていない場所 (境界) に触れている。
    pub fn notice(&mut self, world: &World, dug: &[CellPos]) {
        let mut seen: BTreeSet<CellPos> =
            self.falling.iter().flat_map(|c| c.cells.iter().copied()).collect();
        for &center in dug {
            for (dx, dy) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
                let seed = CellPos::new(center.x + dx, center.y + dy);
                if seen.contains(&seed) || Self::solid(world, seed) != Some(true) {
                    continue;
                }
                let Some(cells) = Self::flood(world, seed, &mut seen) else { continue };
                if self.falling.len() < MAX_FALLING {
                    let mut cells = cells;
                    cells.sort_by_key(|p| (std::cmp::Reverse(p.y), p.x));
                    self.falling.push(Cluster { cells });
                }
            }
        }
    }

    /// `seed` とつながる固体を集める。支えがあるなら `None`。調べたセルは `seen` に入れる (同じ場所を二度調べない)。
    fn flood(world: &World, seed: CellPos, seen: &mut BTreeSet<CellPos>) -> Option<Vec<CellPos>> {
        let mut cells = Vec::new();
        let mut stack = vec![seed];
        let mut local = BTreeSet::new();
        local.insert(seed);
        let mut supported = false;
        while let Some(pos) = stack.pop() {
            cells.push(pos);
            if cells.len() >= MAX_CLUSTER {
                supported = true;
                break;
            }
            for (dx, dy) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
                let next = CellPos::new(pos.x + dx, pos.y + dy);
                if local.contains(&next) {
                    continue;
                }
                match Self::solid(world, next) {
                    None => supported = true,
                    Some(true) => {
                        local.insert(next);
                        stack.push(next);
                    }
                    Some(false) => {}
                }
            }
        }
        // 支えがあるときは、調べ終えた分を覚えて二度調べない。落とすときは覚えなくてよい (すぐ動く)。
        if supported {
            seen.extend(local);
            None
        } else {
            Some(cells)
        }
    }

    /// `p` の 1 つ下へ進めるか (かたまり自身か、空・液体・気体)。固体・粉体・剛体・境界はふさぐ。
    fn can_fall(world: &World, own: &BTreeSet<CellPos>, p: CellPos) -> bool {
        let below = CellPos::new(p.x, p.y + 1);
        if own.contains(&below) {
            return true;
        }
        let cell = world.get(below);
        if cell.material == MaterialId::BOUNDARY || cell.has(FLAG_BODY) {
            return false;
        }
        !matches!(world.materials().get(cell.material).phase, Phase::Solid | Phase::Powder)
    }

    /// 落ちているかたまりを 1 セル落とす。下が空・液体・気体なら入れ替え、ふさがれたら止まる (そこで固体に戻る)。
    pub fn step(&mut self, world: &mut World) {
        self.falling.retain_mut(|cluster| {
            // 固体でなくなった (掘られた・燃えた等) セルがあれば、そのかたまりは追わない。
            if cluster.cells.iter().any(|&p| Self::solid(world, p) != Some(true)) {
                return false;
            }
            let own: BTreeSet<CellPos> = cluster.cells.iter().copied().collect();
            if !cluster.cells.iter().all(|&p| Self::can_fall(world, &own, p)) {
                return false;
            }
            // 下の行から順に 1 つ下と入れ替える (下のセルは先に動いたあと)。
            for p in &mut cluster.cells {
                let below = CellPos::new(p.x, p.y + 1);
                world.swap(*p, below);
                *p = below;
            }
            true
        });
    }
}
