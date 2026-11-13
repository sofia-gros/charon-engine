//! ブロックの設置と破壊 (仕様 17 §3・§4)。ブロックは 8×8 セルで、セルより上位にある。
//!
//! 置くと、その場にあったセルは**消えずに外へ押し出される** (左右 → 上下 → いちばん近い空き)。敵・プレイヤーも、
//! 左右の近い方へ飛ばされる。すべてか、何もしないか: 失敗したら世界もエンティティも変えない (計画を作ってから書く)。
//! セルを動かすのは `World::set` だけ (直接書き換えない)。整数だけで、決定的。

use std::collections::{BTreeMap, BTreeSet};

use charon_core::cell::{Cell, FLAG_BLOCK, FLAG_BODY, FLAG_FALLING, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::Phase;
use charon_core::world::World;

use crate::{EntityId, EntityWorld, collides, in_ellipse};

/// ブロックの一辺 (セル)。
pub const BLOCK_SIZE: i32 = 8;
/// ブロックのセルの数。
pub const BLOCK_CELLS: usize = 64;
/// 押し出しで、行・列に沿って探す距離 (セル)。
pub const SHOVE_REACH: i32 = 64;
/// エンティティを動かすときに探す距離 (1/16 セル)。
const ENTITY_REACH_16: i64 = 64 * 16;

/// ブロックの位置 (ブロック座標)。セルの座標を 8 で割って (床に向けて) 丸めたもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockPos {
    pub bx: i32,
    pub by: i32,
}

impl BlockPos {
    /// セルを含むブロック。
    pub fn containing(cell: CellPos) -> Self {
        Self { bx: cell.x >> 3, by: cell.y >> 3 }
    }

    /// 左上のセル。座標が大きすぎて `i32` に収まらなければ `None`。
    pub fn origin(self) -> Option<CellPos> {
        Some(CellPos::new(self.bx.checked_mul(BLOCK_SIZE)?, self.by.checked_mul(BLOCK_SIZE)?))
    }

    /// 64 個のセル (y の小さい順、x の小さい順)。
    pub fn cells(self) -> Vec<CellPos> {
        let Some(o) = self.origin() else { return Vec::new() };
        (0..BLOCK_SIZE)
            .flat_map(|dy| (0..BLOCK_SIZE).map(move |dx| CellPos::new(o.x + dx, o.y + dy)))
            .collect()
    }
}

/// 置けない理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlaceError {
    /// ブロックのチャンクが読み込まれていない (または座標が大きすぎる)。
    Unloaded,
    /// ブロックにできない材質 (固体・粉体だけ)。
    InvalidMaterial,
    /// 押しのけられないセル (固体の地形・ほかのブロック・剛体・境界) がある。先に掘る。
    Occupied,
    /// 押し出し先がないエンティティがいる。
    EntityStuck(EntityId),
    /// 押し出したセルの行き先がない。
    NoRoom,
}

impl std::fmt::Display for PlaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unloaded => write!(f, "ブロックの場所が読み込まれていません"),
            Self::InvalidMaterial => write!(f, "ブロックにできる材質は、固体と粉体だけです"),
            Self::Occupied => write!(f, "固体のセルやほかのブロックがあって置けません"),
            Self::EntityStuck(id) => write!(f, "エンティティ {} の行き先がありません", id.0),
            Self::NoRoom => write!(f, "押し出したセルの行き先がありません"),
        }
    }
}

impl std::error::Error for PlaceError {}

/// 置いた結果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Placed {
    /// 押し出して動かした (空でない) セルの数。
    pub pushed_cells: usize,
    /// 飛ばしたエンティティの数。
    pub pushed_entities: usize,
}

/// 壊せない理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BreakError {
    Unloaded,
    /// 64 セルが、同じ材質の `FLAG_BLOCK` で揃っていない (欠けたブロックは普通のセル)。
    NotABlock,
}

/// セルの分類。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// 空気。
    Empty,
    /// 押せる (粉体・液体・気体)。
    Movable,
    /// 押せず、通れない (固体・ブロック・剛体・境界)。
    Stop,
}

/// 計画の中の世界: 変更の予定 (`plan`) を重ねて読む。ブロックの 64 セルは `Stop` として扱う。
struct View<'a> {
    world: &'a World,
    plan: BTreeMap<CellPos, Cell>,
    inside: BTreeSet<CellPos>,
}

impl View<'_> {
    fn get(&self, pos: CellPos) -> Cell {
        self.plan.get(&pos).copied().unwrap_or_else(|| self.world.get(pos))
    }

    fn kind(&self, pos: CellPos) -> Kind {
        if self.inside.contains(&pos) {
            return Kind::Stop;
        }
        let cell = self.get(pos);
        if cell.material == MaterialId::EMPTY && !cell.has(FLAG_BODY) {
            Kind::Empty
        } else if cell.has(FLAG_BODY) || cell.is_solid(self.world.materials()) {
            Kind::Stop
        } else {
            Kind::Movable
        }
    }
}

/// 境界のそばから外へ向かう直線の `i` 番目 (0 = ブロックに接するセル)。
type Line<'a> = &'a dyn Fn(i32) -> CellPos;

/// 行 (列) を押す: `incoming` (座標の小さい順の、押し出すセル) を、`line` に沿って外へ押す。
/// 空きが `incoming.len()` 個に届く範囲まで探し、範囲の空でないセルを、順番を保って、ブロックの縁にぴったり詰める。
/// `ascending` は、`i` が増えると座標も増える向き (右・下) か。できたら真 (計画を変える)。できなければ偽 (何も変えない)。
fn shove(view: &mut View<'_>, line: Line<'_>, incoming: &[Cell], ascending: bool) -> bool {
    let k = incoming.len();
    if k == 0 {
        return true;
    }
    let mut empties = 0;
    let mut end = None;
    for i in 0..SHOVE_REACH {
        match view.kind(line(i)) {
            Kind::Empty => {
                empties += 1;
                if empties == k {
                    end = Some(i);
                    break;
                }
            }
            Kind::Movable => {}
            Kind::Stop => return false,
        }
    }
    let Some(end) = end else { return false };
    // 範囲の、空でないセルを、座標の小さい順に並べる。`ascending` なら i の小さい順、そうでなければ大きい順。
    let indices: Vec<i32> = if ascending { (0..=end).collect() } else { (0..=end).rev().collect() };
    let run: Vec<Cell> = indices
        .iter()
        .map(|&i| view.get(line(i)))
        .filter(|c| !(c.material == MaterialId::EMPTY && !c.has(FLAG_BODY)))
        .collect();
    let sequence: Vec<Cell> = if ascending {
        incoming.iter().chain(run.iter()).copied().collect()
    } else {
        run.iter().chain(incoming.iter()).copied().collect()
    };
    // 範囲を空にして、縁 (i = 0) から詰めて並べ直す。
    for i in 0..=end {
        view.plan.insert(line(i), Cell::default());
    }
    for (j, cell) in sequence.iter().enumerate() {
        let i = if ascending { j as i32 } else { sequence.len() as i32 - 1 - j as i32 };
        view.plan.insert(line(i), cell.with(FLAG_FALLING, true));
    }
    true
}

/// 楕円が、ブロックのセルのどれかに重なるか。
fn overlaps_block(x16: i64, y16: i64, width: u8, height: u8, block: &BTreeSet<CellPos>) -> bool {
    let left = x16.div_euclid(16);
    let top = y16.div_euclid(16);
    let right = (x16 + i64::from(width) * 16 - 1).div_euclid(16);
    let bottom = (y16 + i64::from(height) * 16 - 1).div_euclid(16);
    for y in top..=bottom {
        for x in left..=right {
            let (Ok(cx), Ok(cy)) = (i32::try_from(x), i32::try_from(y)) else { continue };
            if block.contains(&CellPos::new(cx, cy)) && in_ellipse(x16, y16, width, height, x, y) {
                return true;
            }
        }
    }
    false
}

/// エンティティを、ブロックの外へ動かす位置 (x16, y16)。左右の近い方 → 反対 → 上下の近い方 → 反対。なければ `None`。
fn escape(
    entity: &crate::Entity,
    world: &World,
    block: &BTreeSet<CellPos>,
    center: (i64, i64),
) -> Option<(i64, i64)> {
    let (w, h) = (entity.width, entity.height);
    let blocked = |x16: i64, y16: i64| {
        collides(x16, y16, w, h, world) || overlaps_block(x16, y16, w, h, block)
    };
    let (cx, cy) = (entity.x16 + i64::from(w) * 8, entity.y16 + i64::from(h) * 8);
    let first_x = if cx < center.0 { -1 } else { 1 };
    let first_y = if cy > center.1 { 1 } else { -1 };
    // 1/16 セルずつ進む。途中で地形にぶつかったら、その向きは行き止まり (壁を通り抜けて、向こう側へは出ない)。
    // ブロックとは、重なったまま進んでよい (これから出るため)。
    for dir in [first_x, -first_x] {
        for d in 1..=ENTITY_REACH_16 {
            let x16 = entity.x16 + dir * d;
            if collides(x16, entity.y16, w, h, world) {
                break;
            }
            if !blocked(x16, entity.y16) {
                return Some((x16, entity.y16));
            }
        }
    }
    for dir in [first_y, -first_y] {
        for d in 1..=ENTITY_REACH_16 {
            let y16 = entity.y16 + dir * d;
            if collides(entity.x16, y16, w, h, world) {
                break;
            }
            if !blocked(entity.x16, y16) {
                return Some((entity.x16, y16));
            }
        }
    }
    None
}

/// ブロックを置く (仕様 17 §3)。`material` は固体か粉体。成功したら、64 セルが旗つきで並ぶ。
pub fn place_block(
    world: &mut World,
    entities: &mut EntityWorld,
    at: BlockPos,
    material: MaterialId,
) -> Result<Placed, PlaceError> {
    // --- 前提 ---
    if material == MaterialId::EMPTY
        || material == MaterialId::BOUNDARY
        || !matches!(world.materials().get(material).phase, Phase::Solid | Phase::Powder)
    {
        return Err(PlaceError::InvalidMaterial);
    }
    let Some(origin) = at.origin() else { return Err(PlaceError::Unloaded) };
    if origin.x.checked_add(BLOCK_SIZE).is_none() || origin.y.checked_add(BLOCK_SIZE).is_none() {
        return Err(PlaceError::Unloaded);
    }
    let cells = at.cells();
    if world.chunk(origin.chunk()).is_none() {
        return Err(PlaceError::Unloaded);
    }
    for &pos in &cells {
        let cell = world.get(pos);
        if cell.has(FLAG_BODY) || cell.is_solid(world.materials()) {
            return Err(PlaceError::Occupied);
        }
    }
    let inside: BTreeSet<CellPos> = cells.iter().copied().collect();

    // --- エンティティ (ID の昇順): 先に行き先を決める ---
    let block_center = (i64::from(origin.x) * 16 + 64, i64::from(origin.y) * 16 + 64);
    let mut moves: Vec<(EntityId, i64, i64)> = Vec::new();
    for entity in entities.iter() {
        if !overlaps_block(entity.x16, entity.y16, entity.width, entity.height, &inside) {
            continue;
        }
        match escape(entity, world, &inside, block_center) {
            Some((x16, y16)) => moves.push((entity.id, x16, y16)),
            None => return Err(PlaceError::EntityStuck(entity.id)),
        }
    }

    // --- セルの押し出し (計画) ---
    let mut view = View { world, plan: BTreeMap::new(), inside };
    let mut pushed = 0usize;
    let mut pending: Vec<(CellPos, Cell)> = Vec::new();
    for dy in 0..BLOCK_SIZE {
        let y = origin.y + dy;
        let row_cells = |range: std::ops::Range<i32>| -> Vec<Cell> {
            range
                .map(|dx| view.world.get(CellPos::new(origin.x + dx, y)))
                .filter(|c| c.material != MaterialId::EMPTY || c.has(FLAG_BODY))
                .collect()
        };
        let (left, right) = (row_cells(0..4), row_cells(4..8));
        pushed += left.len() + right.len();
        // 左半分は左へ: 左端の左から、左へ。
        let x_left = origin.x - 1;
        let left_line = |i: i32| CellPos::new(x_left - i, y);
        if !shove(&mut view, &left_line, &left, false) {
            for dx in 0..4 {
                let pos = CellPos::new(origin.x + dx, y);
                let cell = view.world.get(pos);
                if cell.material != MaterialId::EMPTY {
                    pending.push((pos, cell));
                }
            }
        }
        // 右半分は右へ。
        let x_right = origin.x + BLOCK_SIZE;
        let right_line = |i: i32| CellPos::new(x_right + i, y);
        if !shove(&mut view, &right_line, &right, true) {
            for dx in 4..8 {
                let pos = CellPos::new(origin.x + dx, y);
                let cell = view.world.get(pos);
                if cell.material != MaterialId::EMPTY {
                    pending.push((pos, cell));
                }
            }
        }
    }
    // 左右が塞がっていた分は、上下へ: 上半分は上へ、下半分は下へ (x の小さい順の列ごと)。
    let mut rest: Vec<(CellPos, Cell)> = Vec::new();
    for dx in 0..BLOCK_SIZE {
        let x = origin.x + dx;
        let column: Vec<&(CellPos, Cell)> = pending.iter().filter(|(p, _)| p.x == x).collect();
        let up: Vec<(CellPos, Cell)> =
            column.iter().filter(|(p, _)| p.y < origin.y + 4).map(|&&e| e).collect();
        let down: Vec<(CellPos, Cell)> =
            column.iter().filter(|(p, _)| p.y >= origin.y + 4).map(|&&e| e).collect();
        let y_top = origin.y - 1;
        let up_line = |i: i32| CellPos::new(x, y_top - i);
        let up_cells: Vec<Cell> = up.iter().map(|e| e.1).collect();
        if !shove(&mut view, &up_line, &up_cells, false) {
            rest.extend(up);
        }
        let y_bottom = origin.y + BLOCK_SIZE;
        let down_line = |i: i32| CellPos::new(x, y_bottom + i);
        let down_cells: Vec<Cell> = down.iter().map(|e| e.1).collect();
        if !shove(&mut view, &down_line, &down_cells, true) {
            rest.extend(down);
        }
    }
    // 上下も塞がっていた分は、上下左右でいちばん近い空きへ (同じ距離なら、左・右・上・下の順)。
    for (pos, cell) in rest {
        let mut best: Option<(i32, CellPos)> = None;
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let mut p = pos;
            for step in 1..=SHOVE_REACH + BLOCK_SIZE {
                p = CellPos::new(p.x + dx, p.y + dy);
                if view.inside.contains(&p) {
                    continue; // ブロックの中は通り抜ける。
                }
                match view.kind(p) {
                    Kind::Empty => {
                        if best.is_none_or(|(d, _)| step < d) {
                            best = Some((step, p));
                        }
                        break;
                    }
                    Kind::Movable => {}
                    Kind::Stop => break,
                }
            }
        }
        let Some((_, target)) = best else { return Err(PlaceError::NoRoom) };
        view.plan.insert(target, cell.with(FLAG_FALLING, true));
        view.plan.insert(pos, Cell::default());
    }

    // --- 書き込み (ここまで失敗しなければ、世界を変える) ---
    let plan = view.plan;
    let moved = Placed { pushed_cells: pushed, pushed_entities: moves.len() };
    for (pos, cell) in plan {
        world.set(pos, cell);
    }
    for pos in cells {
        let cell = world.make_cell(material, pos).with(FLAG_BLOCK, true).with(FLAG_FALLING, false);
        world.set(pos, cell);
    }
    for (id, x16, y16) in moves {
        if let Some(entity) = entities.get_mut(id) {
            entity.x16 = x16;
            entity.y16 = y16;
            entity.vx = 0;
        }
    }
    Ok(moved)
}

/// ブロックを壊す (仕様 17 §4): 64 セルがすべて、同じ材質の `FLAG_BLOCK` なら、空にして、その材質を返す。
/// 欠けたブロック (旗が外れたセル) は普通のセルなので、壊せない。
pub fn break_block(world: &mut World, at: BlockPos) -> Result<MaterialId, BreakError> {
    let Some(origin) = at.origin() else { return Err(BreakError::Unloaded) };
    if world.chunk(origin.chunk()).is_none() {
        return Err(BreakError::Unloaded);
    }
    let cells = at.cells();
    let first = world.get(cells[0]);
    if !cells.iter().all(|&p| {
        let c = world.get(p);
        c.has(FLAG_BLOCK) && c.material == first.material
    }) {
        return Err(BreakError::NotABlock);
    }
    for pos in cells {
        world.set(pos, Cell::default());
    }
    Ok(first.material)
}
