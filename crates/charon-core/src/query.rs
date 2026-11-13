//! 読み取り専用の問い合わせ (仕様 01 §7)。セルは書き換えず、整数だけで計算する。

use crate::cell::{Cell, MaterialId};
use crate::coords::CellPos;
use crate::material::Phase;
use crate::world::World;

/// 光線が最初に当たったセル。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RayHit {
    pub pos: CellPos,
    pub cell: Cell,
    /// 当たる 1 つ前のセル (始点が当たったなら始点)。法線の代わりに、置く場所に使う。
    pub before: CellPos,
    /// 始点から数えたセル数 (始点は 0)。
    pub steps: u32,
}

/// 光線が通り抜けられないセルの条件。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stop {
    /// 固体と粉体 (歩けない・止まる材質)。
    Solid,
    /// 空以外のすべて (液体・気体も)。
    NotEmpty,
}

impl World {
    fn stops(&self, cell: Cell, stop: Stop) -> bool {
        if cell.material == MaterialId::BOUNDARY {
            return true;
        }
        let phase = self.materials().get(cell.material).phase;
        match stop {
            Stop::Solid => matches!(phase, Phase::Solid | Phase::Powder),
            Stop::NotEmpty => phase != Phase::Empty,
        }
    }

    /// `from` から `to` へ線を引いて、最初に `stop` に当たるセルを返す (始点も調べる)。
    /// 当たらなければ `None`。Bresenham の線で、浮動小数点数は使わない。
    pub fn raycast(&self, from: CellPos, to: CellPos, stop: Stop) -> Option<RayHit> {
        let dx = (to.x - from.x).abs();
        let dy = -(to.y - from.y).abs();
        let sx = if from.x < to.x { 1 } else { -1 };
        let sy = if from.y < to.y { 1 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (from.x, from.y);
        let mut before = from;
        let mut steps = 0u32;
        loop {
            let pos = CellPos::new(x, y);
            let cell = self.get(pos);
            if self.stops(cell, stop) {
                return Some(RayHit { pos, cell, before, steps });
            }
            if x == to.x && y == to.y {
                return None;
            }
            before = pos;
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
            steps += 1;
        }
    }

    /// 2 点の間が `stop` に遮られていないか (視線が通るか)。
    pub fn line_of_sight(&self, from: CellPos, to: CellPos, stop: Stop) -> bool {
        self.raycast(from, to, stop).is_none()
    }

    /// 中心 `center`、半径 `radius` の円の中のセルのうち、`keep` が真のものを、行 (y) の小さい順、列 (x) の小さい順に返す。
    pub fn cells_in_circle(
        &self,
        center: CellPos,
        radius: i32,
        mut keep: impl FnMut(Cell) -> bool,
    ) -> Vec<CellPos> {
        let r = radius.max(0);
        let mut found = Vec::new();
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy > r * r {
                    continue;
                }
                let pos = CellPos::new(center.x + dx, center.y + dy);
                if keep(self.get(pos)) {
                    found.push(pos);
                }
            }
        }
        found
    }

    /// 円の中の、ある材質のセルの数。
    pub fn count_in_circle(&self, center: CellPos, radius: i32, material: MaterialId) -> usize {
        self.cells_in_circle(center, radius, |c| c.material == material).len()
    }
}
